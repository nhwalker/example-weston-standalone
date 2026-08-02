# Review: PR #17 — Rust migration R1: desktop shell in Rust (hybrid build)

| | |
|---|---|
| **PR** | [#17](https://github.com/nhwalker/example-weston-standalone/pull/17) "Rust migration R1: desktop shell in Rust (hybrid build)" |
| **Merge commit** | `38f1261` (base `b101456`, head `ad98305`) |
| **Merged** | 2026-07-26 |
| **Diff size** | 33 files, +4,831 / −275 |
| **C baseline** | `desktop-shell/shell.c` @ 14.0.1 + trims T1–T9 (2,239 lines) |
| **Reviewed** | all new/changed Rust in full (`westonite-shell` lib+tests, `shell_init.rs`, `desktop.rs`, `grab.rs`, `host.rs`, `curtain.rs`, `layer.rs`, `input_bindings.rs`, `ctx.rs`, `events.rs`, `compositor.rs` additions, plugin crate), line-verified against the corresponding `shell.c` functions; scripts + CI + inventory diffs |

## What the PR does

Ports the trimmed desktop shell to Rust while the frontend stays C: a
safe `westonite-shell` crate holds the policy state machine
(activation/focus/stacking/placement/teardown) against the `ShellHost`
trait; the `weston` fence crate grows the desktop-api vtable, three
pointer grabs + the touch move grab, curtains, pinned layers, input
bindings, per-seat listeners, ref-counted focus tracking, and the
hybrid bootstrap (`shell_init`, dlsym contract); a
`westonite-shell-plugin` cdylib is installed as `desktop-shell.so`.
Adds the destroy-storm stress script (valgrind) and 10 policy unit
tests against a mock host.

## Verdict

This is a high-quality port. The C shell's behavior is reproduced with
line-level care (the resize fixed-point math replicates
`wl_fixed_from_double`'s union trick rather than approximating it; the
busy-grab "mark ungrabbed" quirk and `surface_touch_move`'s *absence*
of a `grabbed` check are both faithfully kept), and the port fixes two
genuine C defects by design: the `bool **` UB in
`has_keyboard_focused_child` (shell.c:962-969 — writing `true` through
a `bool*` that is actually a `bool**` at recursion depth ≥ 2) and the C
leak of a busy `shell_grab` superseded by `weston_pointer_start_grab`
(which never cancels the old grab; C has no path that frees it — the
Rust `retire_superseded_pointer_grab` handles and documents exactly
this). The teardown "graveyard" analysis in `ctx.rs` is the most
careful piece of lifetime reasoning in the migration so far.

The findings are mostly at the seams: promised-but-missing assertions,
documentation drift in the inventory that is supposed to be the audit
surface, sync/deferred tier choices whose user-visible timing effects
went unrecorded, and bookkeeping state that escaped `CtxInner` into
bare thread-locals.

## Findings — correctness

### PR17-C1 (moderate): `dispatch_sync`'s A3-violation fallback is silent — the comment promises a debug assertion that does not exist — **fixed in #41**

`crates/weston/src/ctx.rs:225-248`. The doc comment says a sync-tier
event arriving while the app borrow is held "falls back to enqueueing
… — loud in debug builds." The `None` arm enqueues and nothing else:
there is no `debug_assert!`, no log line. Two consequences:

1. An A3-proof violation (the exact class of bug the callback
   inventory exists to prevent) would be undetectable in any build.
2. The degradation is not semantically neutral: sync-tier events exist
   because their effects must happen *inside* the emitting C frame
   (e.g. `SurfaceRemoved` must release state while the object is
   half-dead; `OutputCreated`'s curtain creation inside the
   `output_created` emission became load-bearing at R2b). A silently
   deferred delivery changes ordering in ways the policy cannot see.

The mid-drain case is legitimately quiet (the test at ctx.rs:396 pins
it) — so the fix is to distinguish "mid-drain" (`draining` flag; quiet)
from "borrow held outside drain" (assert/log). As merged, the two were
indistinguishable. **#41 landed exactly this split** (plus a third
quiet case this review missed: builder-phase dispatch before
`set_app`), keyed on an `in_sync_handler` flag, with the three shapes
pinned by unit tests.

### PR17-C2 (minor): callback-inventory tier cells contradict the implementation they were just updated for — **fixed in #46**

`docs/callback-inventory.md`. The R1 note says every shell-side row is
live, and the PR did update `surface_removed` and `set_parent` to sync
— but left rows whose implementation is *also* sync marked "deferred":

* **L27** (`seat_created`) — table says *deferred*; `register_seat`
  delivers `SeatCreated` via `dispatch_sync` (shell_init.rs:232).
* **L28** (`output_create`) — table says *deferred*;
  `register_output_shell` delivers `OutputCreated` via `dispatch_sync`
  (shell_init.rs:282).

`events.rs` has the same drift internally: `OutputCreated` sits under
the "object lifecycle (**deferred** policy halves)" section header
while being a sync-dispatched event. The inventory's maintenance rule
("growing the sync tier … without a row here fails review") was broken
by the PR that wrote the rule's first status update. Sync-tier growth
is exactly what A3 proofs exist for, so these cells are not cosmetic.

### PR17-C3 (minor divergence, undocumented): `Pong` resolves the client through its first surface; a surfaceless pong strands busy grabs

C `desktop_surface_pong` → `end_busy_cursor(compositor,
desktop_client)` operates on the *client* identity directly
(shell.c:1572-1583). The Rust event carries `surfaces:
Vec<DesktopSurfaceId>` and the policy does
`surfaces.first().map(host.end_busy_grabs_for_client_of)`
(westonite-shell lib.rs:143-147). If every surface of the client was
removed between the pong trampoline and the drain (kill-then-pong
races), `first()` is `None` and no busy grab ends — where C's loop
would still match `grab_client == desktop_client` and end them. The
grab self-heals on the next `busy_focus` pick, so impact is a
transiently stuck busy cursor; but the C shape (client identity in the
event) was available and cheaper.

### PR17-C4 (minor divergence, undocumented): deferred halves of output move/resize split effects C applies atomically

* **Move**: C `handle_output_move` walks background + workspace layers
  in one pass inside the `output_moved` signal (shell.c:2000-2028).
  Rust moves the curtain synchronously in the trampoline
  (`move_backgrounds_with_output`, shell_init.rs:473) but the windows
  only at the next depth-zero drain (`Event::OutputMoved`). Between
  the two, background and windows disagree.
* **Resize**: C `handle_output_resized` recreates the background
  synchronously (shell.c:1914-1923); Rust defers `OutputResized`, so
  the curtain has the stale geometry until the drain.

In the hybrid build the drain follows within the same C dispatch, so
nothing is visible in practice — but this is precisely the class of
event-sequencing divergence this review is chartered to record, and
neither the code nor the inventory notes it. (The inventory files both
under "deferred" *tier*, which describes the mechanism, not the
observable split from the sync half.)

### PR17-C5 (minor): the R0 drain defect (PR16-C1) now has real payloads riding on it — masked by the hybrid build

All of R1's deferred events (`TrackedSurfaceGone` → focus-replacement
hunt, `PingTimeout` → busy cursors, `GrabEnded`, `OutputMoved`,
`OutputResized`, plus every `defer_drop`ed grab/listener box) drain at
depth zero. In the hybrid build that works because *C's*
`wl_display_run` dispatches our trampolines at base depth 0. In the
pure-Rust path (`Compositor::run`, still wrapped in `with_depth` from
PR #16) none of this drains until exit. R1 didn't introduce the bug,
but it stacked the shell's correctness on top of it without a test
covering the pure-Rust composition — the gap surfaced at #24/#25 and
was fixed in #26. **Fixed in #26.**

### PR17-C6 (minor): shell bookkeeping state lives in bare thread-locals outside `CtxInner`

`SEAT_RECS` (shell_init.rs:103-109) and `TRACK_COUNTS`
(compositor.rs:559-568) are module-level thread-locals, unlike every
other piece of wrapper state, which lives in `CtxInner`. The
`TRACK_COUNTS` comment itself documents the hazard this creates
(entries orphaned by one Ctx's teardown bleeding into the next Ctx's
re-minted ids) and the reset-on-fresh-insert workaround for it.
`SEAT_RECS` relies on the shell-destroy handler draining it — correct
today, but the invariant is enforced by convention across two files
rather than by ownership. Both maps belong in `CtxInner`, where
teardown clears them for free and the workaround (and its subtle
comment) disappears. As merged this is a fragility, not a bug.

### PR17-C7 (minor): XOR-derived `own_listener` keys can collide with real object addresses

`shell_init.rs:403,420,518,544` keys the compositor-level listeners as
`ec as usize ^ 1 … ^ 6`. Keys in `ctx.inner.listeners` are otherwise
*object addresses*, and `retire_listener(key)` retires the first
match. `ec ^ 2`, `ec ^ 4`, `ec ^ 6` are even and therefore valid
addresses of other heap objects in principle; a collision with a
registered output/surface address would detach the wrong listener.
Practically unreachable (would require an allocation landing exactly
on `ec^2k`), but the scheme is undocumented cleverness where a
two-variant key enum (`Addr(usize)` / `Slot(u8)`) would be obviously
collision-free.

### PR17-C8 (nit): `Event::GrabEnded` is emitted and consumed by no one

`grab.rs:525,553` enqueue it; the shell's `_ => {}` arm swallows it;
no test asserts it. C has no analogue (grab end is invisible to
policy). Either a consumer is coming (say so where it's emitted) or it
is speculative API — at R1 it reads like a leftover plan artifact.

### PR17-C9 (nit): `create_desktop` failure leaves a half-initialized shell whose destroy listener will still run

`shell_init_body` returns `false` after the destroy listener, `Ctx`,
layers exist (shell_init.rs:424). The C frontend then exits and
`weston_compositor_destroy` fires the full teardown handler with no
app installed — which works (dispatch_sync of `Shutdown` no-ops on a
missing app) but only by accident of the `None` arm's enqueue-then-
discard. A comment stating that the failure path is *intentionally*
covered by the destroy listener would keep someone from "fixing" it.

## Behavioral divergences from the original C

Documented in code/plan (sound, listed for the record):

* **`bool **` UB fixed by design** (plan §10.3):
  `sync_activated`/`has_focused_descendant` implement the evident
  intent of C's `has_keyboard_focused_child`, whose recursion passes
  `&has_keyboard_focus` (a `bool**`) as the `bool*` user_data —
  undefined behavior at depth ≥ 2.
* **Activation child-chain cycle guard** (lib.rs:479-492): C's
  `activate()` recurses unboundedly on `get_last_child`; a
  client-constructed parent cycle overflows the C stack. Rust caps at
  64 hops. Same guard in `sync_activated`/`has_focused_descendant`.
  Unit-tested (`parent_cycle_activation_terminates`).
* **Superseded busy grab retired, not leaked**: C's
  `weston_pointer_start_grab` never cancels the old grab, and a busy
  grab superseded by a move/resize grab has no C free path — a real
  leak. `retire_superseded_pointer_grab` + the current-grab check in
  `end_busy_grabs_for_client_of` handle it, with a snapshot of
  registry-live pointers so a dangling `grab->pointer` is never
  dereferenced.
* **Ping-timeout busy cursor also covers subsurfaces** (lib.rs:127-133,
  documented in-line): C's `get_shell_surface(pointer->focus->surface)`
  returns NULL for a subsurface and silently skips it.
* **Placement PRNG**: xorshift64* instead of libc `random()` — both
  unseeded/deterministic; bit parity explicitly waived (§10 cosmetic).
* **Teardown parking (GRAVEYARD)**: boxes whose trampoline frames are
  on the stack at compositor destroy are parked for process life; the
  C shell leaks the same allocations at the same point.
* **Second `wet_shell_init` returns success without initializing** —
  faithfully mirrors C's odd `return 0` for the already-initialized
  case (shell.c:2184-2189), including never constructing a second Ctx.

Undocumented (the findings above): C1 (sync fallback reordering), C3
(pong via first surface), C4 (move/resize split halves), C5 (pure-Rust
drain gap), plus one small one worth naming: `notify_session` in C
leaves its `focus_state` untouched while the Rust handler re-points
the per-seat tracked reference at whatever took focus — the code
comment argues equivalence-on-death, and the argument is convincing,
but it *is* a state divergence C doesn't have.

Verified equivalences worth recording (checked line-by-line, no
finding): resize fixed-point math (`wl_fixed_from_double` union trick,
truncation on negative division, min/max clamp order); move/touch
delta math including `(int)` truncation for touch; `surface_move`'s
`grabbed` check present and `surface_touch_move`'s absent — both as in
C; busy-grab "mark ungrabbed" quirk kept; click/touch binding
default-grab and focus guards; committed's offset/resize-edge block
including the LEFT/TOP adjustments; map→raise→activate-per-seat order;
initial placement including the no-output `10 + rand()%400` fallback
and the `range > 0` conditions; focus-replacement hunt ordering
(guaranteed by `SurfaceRemoved` being sync and `TrackedSurfaceGone`
deferred); `shsurf->output` tracking correctly identified as dead
state in the trimmed shell (written, never read) and dropped.

## Findings — style / understandability

### PR17-S1: `register_seat`'s "run caps once by hand" block is a maze

`shell_init.rs:207-229`: `let rec_caps_fired = SEAT_RECS.with(|m|
m.borrow().get(&key).map(|_| ()));` is always `Some` (the entry was
inserted eight lines earlier), so the `if` is dead scaffolding — and
the block then re-implements the caps-handler body inline instead of
sharing it. C solves this by literally calling
`shell_seat_caps_changed(&shseat->caps_changed_listener, seat)` once.
Extracting the attach-if-pointer logic into a function called from
both places would delete ~20 lines and the always-true check.

### PR17-S2: the `repr(transparent)` mis-justification propagates

`grab.rs:49-52` copies R0's comment (see PR16-S3): the offset-0 cast
is valid because of `repr(C)` + first field, not because `UnsafeCell`
is `repr(transparent)`. Now in two more places.

### PR17-S3: `events.rs` section headers as tier documentation

The enum groups events under headers like "input tier" / "deferred
policy halves", but tier is actually decided at each dispatch site
(`dispatch_sync` vs `enqueue`). Given C2 showed the headers already
drifted from reality, either the headers should be per-variant doc
comments stating the tier *and the dispatch site*, or tier language
should live only in the inventory. Two sources of truth, one of them
wrong, is worse than one.

### PR17-S4: `westonite-shell` handles `OutputGone` with a bare `..`

lib.rs:176: `Event::OutputGone { .. } => self.reposition_all(host)` —
fine, but directly above it `OutputCreated` also conditionally calls
`reposition_all` for the first output, and C routes both through the
same `shell_output_changed_move_layer` helper. A single comment tying
the two arms to that C function would make the correspondence
checkable; today the reader reconstructs it.

### PR17-S5: magic hop cap `64` appears three times

lib.rs:489, 552, 561 — same guard, same constant, no shared name. A
`const MAX_PARENT_HOPS: u32 = 64;` would also give the "parents are
client-controlled" rationale one home.

## Positive notes

* The **mock-host unit suite** (D20) tests behavior C can't test:
  `parent_cycle_activation_terminates` pins the cycle guard,
  `multi_seat_shared_focus_keeps_other_seats_tracking` pins the
  ref-count contract with an explicit `track_count` probe on the mock,
  and `resize_edge_commit_offsets_match_c_signs` pins the one piece of
  sign-sensitive arithmetic in `committed`.
* `rust-stress-test.sh` asserts a **non-zero mapped-client count**
  before accepting a valgrind-clean run — a "the test must have tested
  something" guard most stress scripts lack — plus a teardown watchdog.
* The `end_busy_grabs_for_client_of` snapshot discipline (busy grabs
  *and* registry-live pointers snapshotted before any mutation, with
  the dangling-`pointer`-field hazard written out) is model §3l code.
* Registry generations now start at 1 and skip 0 on wrap
  (registry.rs) — closing both the forged-zero-id hole and the
  theoretical wrap-alias from PR16-C11, with a test.
* The hybrid log-handler hazard (plugin must not call
  `weston_log_set_handler` or it hijacks the frontend's `--log`) was
  found live, fixed, and recorded in three places.

## Cross-references

* PR16-C1 (drain gap) → carried here as PR17-C5, fixed in #26.
* PR16-S3 (`repr(transparent)` comment) → propagated (PR17-S2).
* #18 (post-merge review) hardens the fence-check for the new crates
  (transitive walk), makes the RPM actually ship this Rust shell, and
  adds the per-seat pointer-destroy guard listener — reviewed there.
* PR17-C1 (dispatch_sync assert): was verified still missing on
  `main` at review time — since **fixed in #41**.
* PR17-C2 (inventory tiers): the L27/L28 "deferred" cells were still
  present on `main` at review time — since **fixed in #46** (sync
  tiers with A3 proofs, dated correction note, `events.rs` grouping).
