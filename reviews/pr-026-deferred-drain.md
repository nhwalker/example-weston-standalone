# Review: PR #26 — rust: drain deferred events at the loop's edges, not at exit

| | |
|---|---|
| **PR** | [#26](https://github.com/nhwalker/example-weston-standalone/pull/26) "rust: drain deferred events at the loop's edges, not at exit" |
| **Merge commit** | `7c66114` (base `c148656`, head `628c619`) |
| **Merged** | 2026-07-29 |
| **Diff size** | 8 files, +294 / −84 |
| **Reviewed** | full diff; the fix, the entry-point audit, and the signal-callback restructuring against C `sigchld_handler`/`on_term_signal` |

## What the PR does

Fixes the dispatch-core defect introduced in #16 (PR16-C1): `run()` no
longer wraps `wl_display_run` in `with_depth` — the loop's base depth
returns to zero, so every trampoline's own 0→1→0 wrap drains at its
edge, exactly where C ran the equivalent handler. The two bare
event-loop signal callbacks (`on_sigchld`, `on_term_signal`) now wrap
their own bodies instead, so one handler means one drain. The fix is
proven with a red-first e2e parity test
(`test_focus_moves_to_survivor_when_focused_window_closes`: PASS on
the C oracle, FAIL on the unfixed Rust frontend at a 10.5 s timeout,
PASS after — 0.59 s) and an audit of every C→Rust entry point.

## Verdict

This is the right fix, correctly scoped, and the engineering
discipline around it is the best in the series:

* **Proven as a user-visible parity bug before fixing** — the test
  encodes the C behavior (`focus_state_surface_destroy` → survivor
  takes focus) and demonstrates the Rust divergence empirically, so
  the fix's value isn't an internal-consistency argument.
* **The audit is written where it will be read**: the `run()` comment
  now enumerates every deliberately-unwrapped entry point *with the
  reason each is correct as-is* (signal callbacks wrap their own
  bodies; label/no-op/log-sink entries must never drain — draining
  from the log sink would run app handlers on a log call;
  `tramp_get_position` makes no outbound call; `client_surfaces`'
  collector isn't an entry point on C's schedule). It even records the
  methodology correction — the fixed-window scan missed two entries
  the body-scoped rescan found — so the next auditor doesn't repeat
  the mistake.
* **The review round caught a regression the first cut introduced**:
  taking `Ctx::current()` before the reap and early-returning without
  it silently turned "no Ctx" into "never call waitpid" — zombies for
  a wrapper-state problem. The merged shape reaps unconditionally
  (the one thing a SIGCHLD handler may never skip) and gates only the
  bookkeeping and the depth wrap on the Ctx, with a `debug_assert` for
  the teardown-ordering bug a missing Ctx would signal. The stale
  comments in `xwayland.rs`/`screenshooter.rs` that had justified
  their wraps by "run() holds the loop at +1" were both rewritten.
* **The de-risking argument is honest**: the hybrid build has
  dispatched these trampolines from C's own loop at base depth zero
  since R1, so drain-at-the-edge had been under destroy-storm stress
  all along; the fix makes the pure-Rust frontend match the hybrid,
  and `rust-stress-test.sh` gained `WESTONITE_BIN` so the storms run
  against both, valgrind-clean.

## Findings

### PR26-C1 (process, for the ledger): three slices elapsed between discovery and fix

The gap was empirically discovered at #24 (and codified as a plan
note), measured as a leak at #25, and fixed here after it was proven
user-visible. Each intermediate step was individually defensible —
and #25's single-listener design decision, made *because* of the gap,
remains correct after the fix — but the arc is worth recording: an
internal-invariant violation ("drains don't happen where the design
says they do") waited for an end-to-end symptom before it ranked as a
bug. The counter-argument (fix-with-proof beats fix-on-suspicion) is
also real; the risk is the two slices shipped in between were built
against the broken timing.

### PR26-C2 (nit): the per-iteration Ctx re-check inside `reap` duplicates the outer match

`on_sigchld` resolves `ctx` once, chooses wrapped-or-bare, and the
loop still does `let Some(ctx) = ctx.as_ref() else { continue }` per
reaped pid. Correct (reaping continues, bookkeeping skips), but the
inner check can never disagree with the outer one within a call; a
reader must convince themselves of that. Hoisting the bookkeeping
into an `if let` around the loop body — or a one-line comment noting
the redundancy is deliberate — would remove the puzzle.

### PR26-C3 (nit): `on_term_signal`'s Ctx-less path is subtle and its comment had to be corrected once already

The review round found the first comment claimed the Ctx-less late
signal became "a no-op instead of an unguarded FFI call" while the
code (correctly) still terminated the display. The corrected comment
now matches the code. The lesson generalizes: comments asserting what
a rarely-taken branch does are worth a test where feasible — this
branch ("SIGTERM after teardown began, before source removal") is
currently argued unreachable and has neither.

## Positive notes

* The audit closed with two *new* findings of its own (the empty grab
  vtable entries not spelled `noop_*`; the nested `collect`), i.e. it
  was a real re-derivation, not a rubber stamp of the change's blast
  radius.
* `rust-stress-test.sh` now checks `WESTONITE_BIN` resolves before
  launching valgrind — a typo fails in seconds instead of at the 20 s
  socket timeout.
* The regression-shape note in PROVENANCE — "(1) is a regression the
  wrap introduced … though unreachable today — `Drop` drains
  `signal_sources` before `Ctx::teardown`, checked" — distinguishes
  "wrong" from "reachable", which most postmortems blur.

## Cross-references

* Closes PR16-C1 (and with it the operative halves of PR17-C5,
  PR24-C1, PR25-C1).
* The `run()` audit list is the artifact future entry-point additions
  must update — flagged as the natural place for a lint/CI check
  (an `extern "C"` fn with outbound calls and no wrap) if the fence
  keeps growing.
