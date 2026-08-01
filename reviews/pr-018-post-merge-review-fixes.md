# Review: PR #18 — Post-merge review of the rust-migration PRs (#11, #13–#16): fixes

| | |
|---|---|
| **PR** | [#18](https://github.com/nhwalker/example-weston-standalone/pull/18) "Post-merge review of the rust-migration PRs (#11, #13–#16): fixes for all confirmed findings" |
| **Merge commit** | `97b4161` (base `38f1261`, head `ac65118`) |
| **Merged** | 2026-07-26 |
| **Diff size** | 23 files, +436 / −147 |
| **Reviewed** | full diff (scripts, CI, RPM spec, `shell_init.rs`, `ctx.rs`, `compositor.rs`, Cargo metadata, docs) with the load-bearing claims re-verified against libweston 14.0.1 sources |

## What the PR does

An internal post-merge review round over the R0/R1 + planning PRs,
landing fixes rather than a findings document: the RPM now actually
ships the Rust shell (with vendored offline cargo build), the
fence-check becomes a transitive graph walk with mandatory crate
classification, the bindings-drift gate really runs in CI, smoke legs
assert which shell ran, `cargo --locked` everywhere, a per-seat
pointer-destroy guard listener in the fence, and a set of honesty
corrections to the plan and callback inventory.

## Verdict

This is what a post-merge review PR should look like. Nearly every fix
here closes a finding this review series independently identified in
#16/#17 (see cross-references), and two of its catches are better than
anything in my own pass over the same code:

* **The RPM shipped the C shell while CI tested the Rust one.**
  `rpm/westonite.spec` had no cargo build at all; every release
  artifact silently contradicted the tested configuration. The fix is
  complete: vendored crates as `Source1` (offline `%build`), the same
  overwrite-install the CI scripts perform, and — crucially — a smoke
  assertion (`westonite-shell: Rust shell initialized` marker, plus its
  *negative* under `WESTONITE_C_ORACLE=1`) so the class of bug
  ("module path identical for both shells, so nothing detects a swap")
  cannot recur silently. A new C-oracle CI smoke leg keeps the other
  side exercised.
* **The per-seat pointer-destroy guard.** `weston_seat_release` frees
  the pointer (input.c:4363) *before* emitting the seat destroy signal,
  so #17's seat-destroy-time `pointer_focus.detach()` could do list
  surgery through a freed `focus_signal` head whenever a backend
  releases the seat without first dropping the pointer capability. The
  fix attaches a oneshot guard to `pointer->destroy_signal` — emitted
  at the top of `weston_pointer_destroy` (input.c:1298) while the
  struct is live — and I verified both cited orderings against the
  14.0.1 tree; the reasoning holds, including the note that the C shell
  has the same latent hazard masked by backend release order.

The review that produced this PR also *corrected its own prior
inventory*: L19/L20/L26 turn out to be vestigial fields in the trimmed
C tree (initialized or declared, never attached), which #17's "every
shell-side row is live" note had glossed; the field count moves 28→29
with the `main()` stack-local L7 properly attributed. This is the
inventory doing its job — though see C2 below for what the round still
missed.

## Findings — correctness

### PR18-C1 (minor): the transitive fence walk resolves default features only

`scripts/rust-fence-check.sh` now uses `cargo metadata --locked` (full
resolve) and walks `resolve.nodes` — correct for the current workspace.
Two blind spots worth a comment in the script:

1. The resolve is computed with **default features**. A safe crate
   acquiring a weston-sys path behind a non-default feature (the way
   `weston/testsupport` gates `weston-sys/testsupport` today) would
   pass the fence while CI's clippy job — which enables
   `weston/testsupport` — builds a different graph. `--all-features`
   on the metadata call would close it.
2. Dev-dependencies are *included* in the walk (resolve nodes carry
   them), which is stricter than the written rule — fine, but the rule
   text ("no safe crate may depend on weston-sys") doesn't say
   dev-deps count, and the first person whose test-only dep trips this
   will find no documented answer whether that's intended.

### PR18-C2 (minor): the inventory round fixed the vestigial rows but not the tier cells

The same editing pass that corrected L19/L20/L26 and the counts left
**L27/L28 marked "deferred"** while the #17 implementation dispatches
`SeatCreated` and `OutputCreated` synchronously (see PR17-C2). A tier
audit was exactly the kind of check this round was doing; the drift
survived it and is still on `main`.

### PR18-C3 (nit): fence 2's Cargo.toml check is a brittle text grep

`grep -q '^unsafe_code *= *"forbid"' crates/$c/Cargo.toml` — correct
for the current spelling, but a crate inheriting `[lints] workspace =
true` (the standard mechanism this workspace may well adopt) would
fail the check despite being correctly configured, and the error
message would point someone at adding a duplicate. A note in the
script (or checking `cargo metadata`'s lint output instead) would
future-proof it.

### PR18-C4 (nit): the ASAN leg's `curl | sh` rustup bootstrap remains unpinned

The PR touched `rust-asan-smoke.sh` twice (the `grep -q` SIGPIPE fix —
a nice catch — and `--locked`) but left the unpinned
`curl -sSf https://sh.rustup.rs | sh` supply-chain nit from #16 in
place. Validation-tool-only, but it now sits in a file whose every
other tool is version-pinned.

## Verified claims (no finding)

* **Signal-source leak fix** (`compositor.rs:313-322`): the partial-
  failure path now removes whichever source installed — closes the
  leak half of PR16-C5. (The dead `CompositorError::SignalSource`
  variant remained dead until #19 wired it up.)
* **`regen-bindings.sh` pinning**: version constant asserted against
  `bindgen --version`, output redirectable via `REGEN_BINDINGS_OUT` so
  fence 3 never touches the tree (closes PR16-C7's in-place mutation);
  `FENCE_CHECK_BINDINGS=1` forced in CI with the pinned CLI in the
  image (closes the always-skipped gate).
* **Smoke assertions**: valgrind leg now requires the same markers as
  the plain leg (closes PR16-C8); marker polling replaces fixed sleeps;
  `smoke-test.sh` deletes stale logs first — with the *reason*
  documented (C `fopen("a")` append mode), which is the difference
  between a fix and a superstition.
* **Stress teardown**: background-subshell watchdog replaced with
  poll-then-reap so no stale-PID `kill -9` can outlive the script.
* **Plan honesty**: the `cargo public-api` snapshot check is now
  stated as *not wired into CI* ("do not rely on it") instead of
  claimed as enforced — the plan previously said "all mechanically
  enforced in CI", which was false. Also: the dead
  `weston-sys/hybrid-r1` feature removed, `undocumented_unsafe_blocks`
  extended to the plugin crate, VENDOR.md's references to deleted
  files (T2/T3) corrected.
* **Drain comment** (`ctx.rs:283-291`): the "no C frame is live"
  claim narrowed to the true invariant (the outermost trampoline's
  frame may be live; it has made its last access to any retired box).
  This is the kind of comment fix that prevents a future "optimization"
  from breaking soundness.

## Findings — style

### PR18-S1: `attach_pointer_listeners` cleanup — good, one naming quibble

The refactor deletes #17's always-true `rec_caps_fired` dance
(PR17-S1 — closed) and shares the attach logic. The `guard` parameter
is a `&Listener` while the call sites pass `&Rc<Listener>` via deref —
fine — but the function attaches to *two different signals* with
different lifetimes (focus vs. destroy) and different rearm rules
(focus re-attaches per caps cycle, guard stays armed); the doc comment
covers it, the name (`attach_pointer_listeners`) doesn't hint that the
two attachments are asymmetric.

## Cross-references

* Closes PR16-C7 (fence-check claims), PR16-C8 (valgrind markers),
  PR16-C9 (`--locked`), the leak half of PR16-C5, and PR17-S1 (the
  register_seat maze).
* Does **not** close: PR16-C4 (signal shape), PR16-C6 (bring-up
  order), PR17-C1 (dispatch_sync assert), PR17-C2 (L27/L28 tiers —
  missed by this very round, see C2 above).
* The pointer-destroy guard is a **new** soundness fix with no
  corresponding earlier finding — credited above.
