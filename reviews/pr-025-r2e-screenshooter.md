# Review: PR #25 — rust(R2e): screenshooter + wcap recorder, frontend-owned

| | |
|---|---|
| **PR** | [#25](https://github.com/nhwalker/example-weston-standalone/pull/25) "rust(R2e): screenshooter + wcap recorder, frontend-owned" |
| **Merge commit** | `c148656` (base `1d0770d`, head `badd5bc`) |
| **Merged** | 2026-07-28 |
| **Diff size** | 13 files, +748 / −35 |
| **C baseline** | `frontend/weston-screenshooter.c` (whole file, 153 lines), `wet_client_start`/`wet_get_bindir_path` (main.c) |
| **Reviewed** | `screenshooter.rs` line-by-line against the C; binding registration, authority attach, recorder fallback, bindir resolution, SIGCHLD hook, teardown ordering |

## What the PR does

Ports `frontend/weston-screenshooter.c`: Super+S spawns
`weston-screenshooter` (bindir resolution via
`weston_module_path_from_env` + /usr/bin; wayland socketpair through
`westonite-spawn`), gated by the in-flight-client slot with C's single
reattached client-destroy listener; Super+R toggles
`weston_recorder_start/stop` on the keyboard-focus output with the
first-output fallback; the capture-authority listener authorizes
exactly the spawned client. Ownership moves to the frontend (the §4
fix pulled forward from R3 — C's shell.c:2232 still calls
`screenshooter_create`; behavior identical). The e2e harness gains the
QEMU Extended Key Event encoding so Super+X bindings are reachable
over VNC at all.

## Verdict

Faithful and carefully argued. The three C touchpoints that require
judgment all got the right treatment:

* **The authority attach bypasses C's helper deliberately** —
  `weston_compositor_add_screenshot_authority` writes
  `listener->notify` itself, which would overwrite the `Listener`
  trampoline; attaching by hand to `output_capture.ask_auth` (the very
  signal the helper adds to) keeps the primitive. Verified against the
  helper's behavior; the same reasoning later recurs for `--debug`'s
  authority in R2g, citing this precedent.
* **The single reattached client-destroy listener** matches C's
  embedded-node reuse — and the field comment explains why the
  "obvious" Rust shape (box per press retired via `defer_drop`) was
  wrong: pending-drop only drains at depth zero, which `run()` never
  reaches mid-loop, so a repeatable user action must not mint a box
  per press. The PR's review round verified the reattach cycle live in
  a debug build (5 spawn→die→reattach cycles with the double-attach
  assert armed, valgrind clean).
* **C's recorder fallback bug is fixed, not inherited**:
  `container_of(output_list.next)` with no empty check is a wild
  pointer under `--no-outputs` (weston-screenshooter.c:100 — verified,
  there is no guard); the port logs "no output to record" and no-ops,
  documented as divergence (a).

`bindir_path` copies bytes (not `to_string_lossy` — it becomes argv[0])
and its `buf.get(..len)` is annotated as defensive rather than a panic
fix, because the C helper returns 0 rather than a truncated length —
a distinction the review round verified at compositor.c:10556.

## Findings — correctness

### PR25-C1 (moderate, process): the drain-gap leak was *measured* here — and the fix still waited one more PR — **fixed in #26**

The review round instrumented `defer_drop` and watched the
pending-drop list grow monotonically across a single move-grab e2e
test (len=1,2,3 at depth 2): the R2d "timing note" was now a
demonstrated lifetime-long memory leak. The response was to record the
second dimension in plan §3e/A4 and design *this* PR around the gap
(the single-listener choice above) — correct local engineering, but it
is the second consecutive slice to ship on top of a defect the project
had already characterized. #26 fixed it days later with the
user-visible focus-bug proof. Recorded as the tail of the
PR16-C1 → PR24-C1 → here → #26 arc.

### PR25-C2 (minor divergence, documented): failed spawn topology + the single-pid-slot race

Documented divergence (b): a failed spawn is reported in the parent
(std's status pipe) and creates no `wl_client`, where C forks first
and leaves a doomed client whose death resets the slot — same terminal
state. The corollary is the interesting part and is also documented: C
tracks children on a list, the port keeps one pid slot per subsystem,
so when `wl_client_create` fails and a second Super+S lands before the
first child's SIGCHLD, C logs both exits and the port logs only the
later one. A real (if narrow) observability divergence, correctly
traded for simplicity and written down.

### PR25-C3 (nit): binding tier is sync where the plan said deferred — documented, inventory updated

B1/B2 were planned as deferred-tier pre-R0; they run sync (the
handlers touch only wrapper state — A3 proofs recorded). The
divergence-from-plan is documented as divergence (c) and the
callback inventory rows were actually updated this time (contrast
PR17-C2). No action; noting for the ledger.

### PR25-C4 (nit): recorder stop order inverted vs C — equivalent

C: `weston_recorder_stop(recorder); shooter->recorder = NULL;`. Rust:
`Cell` swap to null, then stop. No observable difference (the binding
is the only reader), but a one-word comment ("swap-first so re-entry
sees no recorder") would make the inversion look intentional rather
than accidental.

## Positive notes

* **The VNC output-capture abort found live** (`assert(wl_list_empty
  (&ci->pending_capture_list))`, output-capture.c:193, reproducible
  with the C frontend once a VNC peer is connected) is an upstream
  RPM-side bug the port *didn't* paper over: it was reproduced under
  the oracle, scoped out per the our-code-only rule, and the e2e
  exercises the spawn path via `WESTON_MODULE_MAP` diversion and the
  deny path on headless instead. The finding is recorded in three
  docs.
* The review round's six-item claim-by-claim verification (including
  admitting the doc-comment splice was the author's own Edit-anchor
  mistake) is the most honest self-review in the series.
* `vncclient.py` gaining qnum keycodes because *the server's keysym
  path never tracks Super* — the harness limitation was diagnosed at
  the protocol level, not worked around with sleeps.
* `handle_child_exit` copies the exe path out of the RefCell before
  the outbound `weston_log` call — the no-borrow-across-FFI rule
  applied at a place where violating it would work 99.9% of the time.

## Cross-references

* PR16-C1 / PR24-C1 → measured here → fixed in #26.
* The frontend-ownership move (screenshooter created by `build()`)
  is the §4 plan fix pulled forward; the C tree keeps shell.c:2232 —
  a deliberate hybrid-phase asymmetry, documented.
* C4 checked against `main`: unchanged (still swap-then-stop, no
  comment).
