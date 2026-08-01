# Review: PR #16 — rust(R0): workspace + weston-sys bindings

| | |
|---|---|
| **PR** | [#16](https://github.com/nhwalker/example-weston-standalone/pull/16) "rust(R0): workspace + weston-sys bindings over the installed 14.0.1 headers" |
| **Merge commit** | `b101456` (base `81cfc1e`, head `4ab9676`) |
| **Merged** | 2026-07-25 |
| **Diff size** | 34 files, +12,851 (of which `bindings.rs` is 10,216 generated lines) |
| **C baseline** | weston 14.0.1 (`61f2248de`) + local patches P0–P4, trims T1–T9 (per VENDOR.md) |
| **Reviewed** | every hand-written file in full; `bindings.rs` spot-checked (header markers, generation provenance) |

## What the PR does

Lands the Cargo workspace and the two foundation crates of the Rust
migration: `weston-sys` (committed bindgen output over the installed EPEL
libweston-14 headers, a pkg-config version tripwire in `build.rs`, and a
`cc`-built C shim for the wayland static inlines + the `weston_log`
va_list pair) and `weston`, the "fence" crate with the §3 primitives:
generational-id registry, `Listener` trampoline, two-tier
dispatch with depth-counted drain, pointer-`Grab` skeleton, panic
barrier, thread-local `Ctx`, `ShellHost` trait, plus a canned headless
bring-up (`compositor.rs`) driving the `r0-smoke` exit-criterion binary.
CI gains rustfmt/clippy/fence-check/valgrind-smoke/ASAN legs.

## Verdict

The primitive layer is genuinely well designed — the generational-id
registry, oneshot self-detach listener discipline, deferred-drop-of-grab
boxes, and panic barrier each neutralize a real C-idiom hazard, and each
is unit-tested against a real `wl_signal` via the fake-C-object harness.
The docs (`r0-header-facts.md`, `callback-inventory.md`) are an unusually
strong review surface.

That said, this PR introduces the single most consequential defect of the
whole migration (C1, the dispatch-depth wrap of `wl_display_run`, which
silently defeated the drain-at-the-edge design for every later slice
until #26), plus a cluster of C-fidelity gaps in the canned bring-up that
later PRs had to walk back (C2, C3, C6).

## Findings — correctness

### PR16-C1 (major): `run()` wraps `wl_display_run` in `with_depth`, so deferred events never drain during the session — **fixed in #26**

`crates/weston/src/compositor.rs:323-327`:

```rust
self.ctx.with_depth(|| {
    unsafe { weston_sys::wl_display_run(display) };
});
```

The crate-level contract (`lib.rs:19-23`) is "whoever returns \[the
dispatch depth\] to zero drains the deferred-event queue and then the
pending-drop list". Wrapping the loop itself holds the depth at ≥1 for
the entire session: every trampoline that fires inside the loop runs at
depth ≥2, returns to 1, and never drains. Consequences:

* Every deferred-tier `Event` (at R0: `OutputGone`, `HeadGone`,
  `CompositorShutdown`…) is delivered to the `EventSink` in one batch
  **after the loop exits** — i.e. at shutdown. The r0-smoke binary's
  sink demonstrably never sees a mid-session event.
* Every `defer_drop` box (grab inners, migrated listener boxes)
  accumulates for the process lifetime — a leak that PR #25's review
  measured growing monotonically, and #26 proved as a user-visible parity
  bug (`test_focus_moves_to_survivor_when_focused_window_closes` failed
  against the Rust frontend, passed against the C oracle).

The unit tests in `ctx.rs` did not catch this because they drive
`with_depth` directly and never run a real event loop — the test suite
pins the primitive's contract but nothing pins the *composition* of the
primitive with `wl_display_run`. Worth remembering as a pattern: the R0
exit criterion (clean SIGTERM exit) was satisfiable with the drain
machinery effectively disabled.

Fixed in PR #26 (leave the loop call unwrapped; wrap the bare event-loop
signal callbacks instead), with an audit of every C→Rust entry point.

### PR16-C2 (major): no equivalent of C's `init_failed` — output bring-up failure leaves a running, zero-output compositor exiting 0 — **fixed in #20**

`crates/weston/src/compositor.rs:469-474`. On output create/configure/
enable failure the code logs (`westonite-r0: enabling output {name}
failed`) and returns; `build()` succeeds. The C frontend sets
`wet->init_failed = true` on each of these paths (`frontend/main.c:2033,
2049, 2058`) and `main()` checks it immediately after
`weston_compositor_flush_heads_changed` (`main.c:4672`) and aborts
startup. The Rust R0 behavior is the exact "silent degradation" failure
mode the project's own fail-loud rule targets. PR #20's review round
added the `init_failed` flag to `Ctx` and C's log wording.

### PR16-C3 (moderate): registry registration + destroy-listener attach happen *before* configure/enable; C attaches its tracker only after a successful enable — **fixed in #20**

`crates/weston/src/compositor.rs:441-444` registers head and output (and
attaches both destroy listeners) right after `weston_compositor_create_
output`, before `output_set_size`/`weston_output_enable`. C's
`simple_head_enable` calls `wet_head_tracker_create` **last**, only after
enable succeeds (`frontend/main.c:2066`). Two observable consequences of
the early registration:

1. On enable failure, `weston_output_destroy` (line 473) fires the just-
   attached destroy listener, which enqueues `Event::OutputGone` for an
   output the app was never told existed (there is no `OutputCreated`
   at R0, but the asymmetry became real at R2b).
2. The registry slot is claimed before `output_created_signal` fires —
   the root cause of the "shell's background curtain never created under
   the Rust frontend" bug that PR #20's review round diagnosed and fixed
   by moving registration after `weston_output_enable`.

### PR16-C4 (moderate divergence): signal-handling shape differs from C, and the code comment misstates it — SIGINT semantics, SIGUSR2, missing log line

`crates/weston/src/compositor.rs:295-314` installs SIGTERM **and
SIGINT** as `wl_event_loop_add_signal` sources. C (`frontend/
main.c:4544-4576`) deliberately does something different:

* SIGTERM and **SIGUSR2** go through `wl_event_loop_add_signal`;
* SIGINT is installed via plain `sigaction`, whose handler
  `raise(SIGUSR2)`s — an explicit workaround so gdb can catch Ctrl+C
  (the comment block at `main.c:4553-4562` explains this);
* SIGCHLD is a loop source; SIGUSR1 is blocked process-wide before any
  backend threads spawn (for xwayland).

At R0 the practical deltas are: `kill -USR2` does not terminate the Rust
compositor cleanly (it kills the process with default disposition), and
Ctrl+C under a debugger exits weston instead of breaking into gdb. Also
missing: C logs `caught signal %d` (`main.c:831`) before terminating;
the Rust handler is silent, so a log-based post-mortem cannot tell a
signal-driven exit from a natural one. Finally, the SAFETY comment on
the Rust code says "as main.c installs for SIGTERM/INT", which is not
what main.c does — the comment actively misleads.

*Status:* SIGCHLD and SIGUSR1-blocking arrive with later slices (R2a,
R2d). Verified against current `main` (post-#36): the SIGINT-via-
signalfd shape, the missing SIGUSR2 route, the missing `caught signal`
log line, and the inaccurate comment **all persist** — this is a live
divergence, not one a later PR cleaned up. Will be examined in detail at
the #19 (R2a) review, which owns the real signal-source port.

### PR16-C5 (minor): partial failure installing signal sources leaks the first source and bypasses the error type

`crates/weston/src/compositor.rs:315-321`: if `s1` succeeds and `s2`
returns null, the early `return 1` happens before either source is
pushed to `self.signal_sources`, so `s1` is never
`wl_event_source_remove`d in `Drop` (harmless in practice — the display
teardown reclaims it — but inconsistent with the "removed in Drop, as
main.c removes its signals[] sources" comment directly above). Related:
`CompositorError::SignalSource` (`compositor.rs:54`) is declared for
exactly this failure and never constructed — `run()` returns a bare `1`
with a log line instead. Either build the sources in `build()` (where
`?` works, and closer to C, which installs signals before anything else)
or drop the dead variant.

### PR16-C6 (minor divergence): socket is bound before the heads flush; C flushes first

`build()` order (`compositor.rs:196-214`): `backends_loaded` → **add
socket** → `flush_heads_changed` → `wake`. C's order
(`frontend/main.c:4660-4767`): `backends_loaded` →
`flush_heads_changed` (+ `init_failed` check) → socket
(`main.c:4706`) → shell/xwayland/modules → `wake`. Since nothing
dispatches until `wl_display_run`, no client can actually connect in
the gap — but the ordering is observable (the socket exists on disk
before outputs are configured) and it bit later: the R2c-vnc review
round found `rust-smoke` leg 7 waiting on the socket as a proxy for
"outputs are up", which is exactly the inference this reordering
invalidates. `wake` before any shell exists is also a (benign at R0)
reordering relative to C, where wake happens after shell + autolaunch
setup. Worth normalizing to C's order when there's no reason not to.

### PR16-C7 (minor): fence-check claims more than it checks — **fixed in #18**

`scripts/rust-fence-check.sh`:

* The header comment (line 5-6) says the dependency rule is enforced
  "directly or transitively", but the implementation reads
  `cargo metadata --no-deps` and inspects only **direct** dependencies
  (line 35). A safe crate depending on an intermediate crate that
  re-exports `weston-sys` would pass. #18 replaced it with a transitive
  graph walk and an unclassified-crate failure.
* Check 3 regenerates `bindings.rs` **in place** and moves the committed
  copy back on mismatch (lines 53-58) — an interrupted run leaves the
  working tree dirty or corrupted. #18 regenerates to a scratch file.
* Check 3 silently self-skips when `bindgen` is absent (line 62) — and
  the CI image at this PR does not install `bindgen-cli`, so the drift
  gate never actually ran in CI. #18 pins `bindgen-cli` in the image and
  forces `FENCE_CHECK_BINDINGS=1` in CI.

### PR16-C8 (minor): the valgrind smoke leg asserts exit status only — **fixed in #18**

`scripts/rust-smoke.sh:34-41`: leg 4 (valgrind) checks only that the
process exits 0; unlike leg 3 it does not require the
`Output 'headless' enabled` / `clean exit (0)` markers. A regression
that makes startup silently do nothing (cf. C2 — a mode this same PR
makes reachable) would still pass the valgrind leg. #18 made the
valgrind leg assert the same markers.

### PR16-C9 (nit): `cargo` invocations are not `--locked` — **fixed in #18**

`rust-smoke.sh`, CI clippy step, `rust-fence-check.sh` (`cargo
metadata`): none pass `--locked`, so CI could silently float
dependencies past `Cargo.lock`. #18 added `--locked` everywhere and
pinned the bindgen version in `regen-bindings.sh`.

### PR16-C10 (nit): a second `Ctx::new()` on one thread silently orphans the first in release builds

`crates/weston/src/ctx.rs:95-99`: the one-live-`Ctx` invariant is a
`debug_assert!`, but the slot is overwritten unconditionally, so in
release a second `build()` while a first `Compositor` is alive would
leave the first compositor's trampolines resolving to the *new* context
(wrong registries, wrong queue). Unreachable from the single R0 call
path, but the invariant deserves a hard error (or documented refusal) at
the `CompositorBuilder::build` boundary rather than a debug-only check.

### PR16-C11 (nit, theoretical): registry generation counter can wrap

`crates/weston/src/registry.rs:85`: `generation.wrapping_add(1)` — after
2³² invalidations of one slot, a hoarded stale id would resolve to a
live foreign object. Unreachable in practice (would require ~4 billion
output destructions); worth a one-line comment acknowledging the
tradeoff, since the whole point of the type is that stale ids *never*
resolve.

## Findings — style / understandability

### PR16-S1: `grab.rs` `dispatch()` takes a `what_suffix` parameter it explicitly discards

`crates/weston/src/grab.rs:62-72`: every trampoline passes a suffix
(`"focus"`, `"motion"`, …) and `dispatch` does `let _ = what_suffix;`.
The panic barrier consequently reports only the grab's overall `what`,
not which vtable entry panicked — the parameter reads as if it adds
context but doesn't. Either fold the suffix into the barrier label or
remove the parameter; the current shape makes the reader hunt for a use
that doesn't exist.

### PR16-S2: `PointerGrabInner.ended` is written twice and never read

`grab.rs:42,121,164`: both `tramp_cancel` and `PointerGrab::end` set
`ended`, but nothing reads it in this PR. Other R0-forward-declarations
carry `#[allow(dead_code)] // R0: … arrives at R1` markers; this one is
invisible dead state — a reader auditing the grab lifecycle has to
discover on their own that the flag is (presumably) for R1. Deserves the
same explicit marker, or deletion until R1 needs it.

### PR16-S3: misleading `repr(transparent)` comments on the offset-0 casts

`listener.rs:26-28` and `grab.rs:39-41` justify the
`wl_listener*`→`ListenerInner*` cast with "`repr(transparent)` keeps the
offset-0 cast valid". `UnsafeCell` being `repr(transparent)` is
necessary but not the load-bearing fact — the cast is valid because
`ListenerInner` is **`repr(C)`** with `raw` as its first field (which
the first sentence of the comment does state). As written, a reader can
come away thinking `repr(transparent)` on the *outer* struct is what
matters. Small, but these are exactly the comments future unsafe-code
reviewers will lean on.

### PR16-S4: the headless windowed-API lookup is duplicated as a magic string

`compositor.rs:268` and `compositor.rs:460` both spell
`c"weston_windowed_output_api_headless_v2"` and separately re-derive the
`weston_plugin_api_get` call. The second lookup (inside `enable_head`)
could reuse the API pointer resolved at `load_headless` time or at least
share a named constant; duplicated magic strings + duplicated
`size_of` arguments is the sort of thing that goes wrong silently when
v3 arrives.

### PR16-S5: shim truncation comment promises information the sink never gets

`crates/weston-sys/shim/shim.c:39-41`: "long lines are truncated, which
the sink can see from the return value of vsnprintf if it ever matters"
— the sink receives only `buf`/`len`/`cont`; `vsnprintf`'s return value
is not forwarded, so the sink *cannot* see truncation. The truncation
math itself is correct (`n < 1024 ? n : 1023` matches the written
bytes); only the comment is wrong.

### PR16-S6: `log_line` appends `\n` implicitly — an easy double-newline trap

`crates/weston/src/log.rs:22`: `weston_log(c"%s\n", …)` appends the
newline, so every future caller must remember *not* to include one —
the opposite of the C convention where every `weston_log` call site
writes its own `\n`. Porting C log lines verbatim will produce blank
lines. A doc-comment sentence on `log_line` ("no trailing newline —
added here") would prevent that; today the behavior is only discoverable
by reading the format string.

## Behavioral divergences from the original C (summary)

Documented/by-design (fine, listed for the record):

* Two-tier dispatch: destroy notifications split into synchronous
  registry invalidation + deferred policy events; C runs policy inside
  the destroy handler. This is the plan's central re-design (§3e), and
  the eager-payload rule (`events.rs`) is the right consequence.
* Panics → logged abort at the trampoline boundary (C has no analogue).
* Re-entrant emission of the *same* listener is silently skipped
  (`listener.rs:73-77` `try_borrow`) — C would re-enter the handler.
  Documented in-line; note it is silent event loss if it ever happens.
* Canned R0 configurator: no `weston_output_lazy_align`, fixed scale
  1 / normal transform, single fixed mode — explicitly documented as
  smoke-only, superseded at R2b.

Undocumented at merge time (the review findings above):

* C1 — deferred tier effectively disabled during the loop (event
  *timing* diverges from both C and the plan's own design).
* C2 — startup succeeds where C's would fail (`init_failed`).
* C3 — destroy-listener attach point (before vs. after enable) and the
  resulting phantom `OutputGone`.
* C4 — SIGINT/SIGUSR2 semantics, no `caught signal` log.
* C6 — socket-before-flush ordering; wake-before-shell.

## Positive notes

* `recycled_slot_does_not_resurrect_old_id` (`registry.rs:133`) pins the
  exact ABA hazard the generational design exists for — the test suite
  tests the *reason*, not just the mechanism.
* The pkg-config version tripwire (`build.rs:20-34`) turning "EPEL
  bumped weston" into a build failure with a regeneration script named
  in the message is exactly how a committed-bindings model stays honest.
* `docs/r0-header-facts.md` re-verifying every §3 claim against the
  *installed* headers (and catching two real deltas: mandatory
  `backends_loaded`, `lazy_align` being frontend-local) is a model
  practice; the F9 note (14.x `get_label` callback vs 15.x `char*`)
  probably prevented a live bug.
* `callback-inventory.md` reconciling its row counts against the plan's
  figures makes the "no unsoundness outside `weston`" claim genuinely
  auditable.

## Cross-references

* #18 fixes C7, C8, C9 (fence-check, smoke assertions, `--locked`).
* #20 fixes C2, C3 (init_failed, registration-after-enable) and
  replaces the canned configurator wholesale.
* #26 fixes C1 (deferred-drain), with the entry-point audit.
* C4 (signal shape) and C6 (bring-up ordering) to be re-checked at #19.
