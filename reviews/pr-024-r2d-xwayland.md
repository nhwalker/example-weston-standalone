# Review: PR #24 — rust(R2d): xwayland — the Rust frontend now runs the entire e2e suite

| | |
|---|---|
| **PR** | [#24](https://github.com/nhwalker/example-weston-standalone/pull/24) "rust(R2d): xwayland — the Rust frontend now runs the entire e2e suite" |
| **Merge commit** | `1d0770d` (base `39b942c`, head `79575fd`) |
| **Merged** | 2026-07-27 |
| **Diff size** | 15 files, +788 / −23 |
| **C baseline** | `frontend/xwayland.c` (whole file, 267 lines), `wet_client_launch`/`sigchld_handler` slices of `main.c` |
| **Reviewed** | `xwayland.rs` line-by-line against `xwayland.c`; `westonite-spawn` additions, build.rs listenfd probe, compositor/sigchld wiring |

## What the PR does

Ports all of `frontend/xwayland.c`: module load + `weston_xwayland_v3`
API resolution + `listen` (every failure fatal at startup, as C);
`spawn_xserver` (socketpairs, `-displayfd` pipe, dup of the module's
borrowed listen fds, fork of `[xwayland] path` through
`westonite-spawn`, `wl_client_create` on the parent wayland end);
`handle_display_fd` (readiness → hand `wm_fd` to `xserver_loaded`);
the SIGCHLD pid-match relaying `xserver_exited` (lazy respawn); and
teardown before `weston_compositor_destroy`. `westonite-spawn` gains
`Command::new/arg/arg_fd` (fd-by-argv-number, the C fdstr contract),
`socketpair_stream`, `pipe_cloexec`. SIGUSR1 is now blocked
process-wide before backend threads spawn (C main.c:4570).

## Verdict

`xwayland.rs` is the cleanest FFI file in the migration: the module
header carries an explicit **ownership map** for every fd crossing the
boundary (borrowed listen fds → dup'd; wayland parent end → transfers
at `wl_client_create` with `mem::forget` on success and drop-close on
failure; `wm_fd` → transfers at `xserver_loaded`; pipe read end →
ours), and I verified each against the C flow including the
error-path close behavior (C's `err`/`err_proc` labels vs. Rust's
OwnedFd drops — equivalent, with C's "unknown child" sigchld outcome
reproduced and its rationale quoted). The `-displayfd` retry protocol
(read-until-newline, EAGAIN re-arm) matches C branch for branch. Three
C fd-hygiene defects are fixed *and documented as divergences* rather
than silently: the EOF-spins-forever pipe (a), the leaked parked
`wm_fd` on respawn (b), and — the subtle one, confirmed by the PR's
own review round with a dispatch-batch analysis — the stale readiness
watch a dead server can leave installed when the module respawns
within the same epoll batch, whose later dispatch would tear down the
*new* server's watch and leave the WM unstarted.

The `_bindgen_ty_2` positional-enum pin (compile-time
`WL_EVENT_READABLE == 0x01` check with an explanation of why a
positional name is a regen hazard) is small-scale rigor worth copying.

## Findings — correctness

### PR24-C1 (moderate, process): the deferred-drain gap was discovered here — and codified as benign instead of filed as a bug — **fixed in #26**

`handle_child_exit`'s comment (xwayland.rs:410-419) justifies its
`with_depth` wrap and then states, from an empirical probe: "no drain
fires anywhere inside the run loop; see the plan §6 note on
deferred-drain timing." That probe is the discovery of PR16-C1's
consequence — mid-run deferred events (and `defer_drop` boxes) never
drain — and this PR's disposition was to *document it as a timing
note* rather than treat it as the defect it was. #25 then measured the
pending-drop list growing monotonically; #26 proved it as a
user-visible focus bug and fixed it, and its audit specifically
rewrote this comment (whose "run() holds the loop at +1" justification
had become stale). Nothing in this PR is wrong per se — but the
evidence for a real bug existed here, one slice earlier than the fix,
and the plan note framed it as expected behavior.

### PR24-C2 (nit): `spawn` failure collapses C's two log lines into one — documented, but the wording invents a C-sounding function name

The comment explains the fork-vs-exec merge honestly, but the merged
line is prefixed `weston_client_launch:` — a function that doesn't
exist in either tree (C's is `wet_client_launch`). Anyone grepping the
C source for the prefix will find nothing. Trivial, but log lines are
this project's parity surface; an invented C-ish prefix is worse than
either exact parity or a plainly Rust-side message.

### PR24-C3 (nit): `handle_child_exit` matches only the Xwayland pid; C's process list generalizes

C keeps a `wet_process` list with per-process cleanup callbacks; the
port keeps one pid slot per subsystem (xwayland here, autolaunch in
`on_sigchld`). Adequate for the current two children and later
*documented* as divergence (c) in the R2e entry (two clients whose
exits race log differently) — noting it here because this PR is where
the single-slot shape was chosen.

## Behavioral divergences — deliberate & documented (all sound)

* (a) displayfd EOF: C returns 1 and re-arms a level-triggered pipe
  that stays readable forever (spinning until the process watcher
  fires); Rust tears the watch down — an EOF'd pipe can never complete
  the line.
* (b) respawn hygiene: parked `wm_fd` closed (C overwrites the number
  and leaks the fd); stale readiness watch removed (C keeps the
  source). The second is the load-bearing one — verified as a real
  same-batch hazard in the PR's review round.
* (c) child-side `seteuid(getuid())` deferred with the R2a audit note
  (euid == uid in every supported deployment).
* Null return from `wl_event_loop_add_fd` is logged (C stores the NULL
  silently and the WM just never starts).

## Positive notes

* The fd **ownership map as module-header doc** — every later fd bug
  in this file will be triaged against a written contract instead of
  re-derived folklore.
* `spawn_xserver` copies the display name as bytes
  (`OsStr::from_bytes`), not `to_string_lossy` — with the comment
  explaining that a replacement character would hand the child a
  display the module isn't listening on. Exactly the §3h edge most
  ports get wrong.
* The review round's five verified claims (stale watch REAL;
  byte-preserved display name; the `_bindgen_ty_2` pin; the A4 wrap
  kept with its hazard honestly re-scoped; the meson citation
  corrected to a file that exists in this repo) are all visible in the
  merged code — the PR's self-review left an audit trail.
* Smoke leg 8 runs the full lazy-spawn → xdpyinfo → WM → SIGTERM
  lifecycle under valgrind, and the live probes (kill -9 respawn with
  launch-count assertion) test the respawn path most ports never
  exercise.
* SIGUSR1 blocking landed at the right place (before backend threads
  spawn, inheriting the mask) — one more piece of PR16-C4's signal
  story closed; SIGUSR2/SIGINT shape remains open.

## Cross-references

* C1 → #25 (measured) → #26 (fixed; comment rewritten there).
* Single-pid-slot shape (C3) → documented at #25 as divergence (c).
* PR16-C4: SIGUSR1 half closed here; the SIGINT/SIGUSR2 half still
  open on main.
