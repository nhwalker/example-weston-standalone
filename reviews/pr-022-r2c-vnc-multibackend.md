# Review: PR #22 — rust(R2c): VNC backend + multi-backend — full e2e suite on westonite-rs

| | |
|---|---|
| **PR** | [#22](https://github.com/nhwalker/example-weston-standalone/pull/22) "rust(R2c): VNC backend + multi-backend — full e2e suite on westonite-rs" |
| **Merge commit** | `ac30317` (base `b65f4f1`, head `0981e15`) |
| **Merged** | 2026-07-26 |
| **Diff size** | 17 files, +905 / −132 |
| **C baseline** | `frontend/main.c`: `load_vnc_backend` (3855), `vnc_backend_output_configure` (3795), `load_backends`, `weston_compositor_init_config`, backends parsing (4458/4602-4606, 4653) |
| **Reviewed** | full diff; `vnc_backend_output_configure` semantics, backends-variable precedence, and `multi_backend` line-verified against `main.c` |

## What the PR does

The Rust frontend gains the VNC backend and true multi-backend
loading: an ordered `BackendSpec` list on the builder, per-backend
head dispatch in `heads_changed` (the `wb->simple_output_configure`
role, as a `HeadSetup` enum), `OutputPolicy::decide_vnc` (640x480
defaults, section-only scale, forced-normal transform, `resizeable`),
the `[keyboard]`/xkb + `repaint-window` slice of
`weston_compositor_init_config`, and `--address`/`[vnc]` plumbing.
`rust-e2e-test.sh` now runs the whole suite minus xwayland against
`westonite-rs` — the first framebuffer-level validation of the Rust
stack (and, notably, the first thing that could have caught PR19-C1).

## Verdict

A strong slice whose review round did the C-fidelity work this series
keeps asking for — three of its fixes are textbook:

* **`--backend`/`--backends` collapsed to C's one variable** (closes
  PR19-C3), with clap `visible_alias` + `overrides_with` for
  last-occurrence-wins, the config-side `backends`-then-`backend`
  order restored, the singular key comma-split, and a test
  (`backend_and_backends_are_one_variable`) that cites the exact
  `main.c` lines for each behavior it pins.
* **`compositor->multi_backend` set** — found by reading libweston's
  consumer (`output_accumulate_damage`, compositor.c:3405): with two
  backends and the flag false, the core drops the buffer reference
  while the second backend still needs it. This is the kind of bug no
  e2e test would attribute correctly; catching it by reading the
  consumer is exactly right.
* **Renderer AUTO passed through unresolved** — the R2a code resolved
  `Auto → Noop` whenever headless was present; C hands AUTO to every
  loader and the first to reach `if (!compositor->renderer)` decides,
  so `--backends=vnc,headless` must come up on pixman, not hand VNC a
  noop it rejects. The comment documents the load-order semantics.

Two catches deserve special mention. The **signal-sources-before-
backend-load** move is a genuinely subtle correctness fix: `wl_event_
loop_add_signal` blocks the signal via `sigprocmask` on the calling
thread, later-spawned threads inherit that mask, and the VNC backend
spawns neatvnc/aml workers during load — install after, and a
process-directed SIGTERM can hit a worker that never blocked it and
kill the process by default action. Diagnosed from a live per-delivery
CI race, and the explanation is now in the code. And the
**C `parsed_options` clobbering bug** (each loader's
`wet_init_parsed_options` replaces the table, so multi-backend C drops
`--width`/`--height`/`--scale`/`--transform` entirely) is *documented
and deliberately not replicated* — the divergence note at
`build_output_policy` is a model of how to handle a C bug you decline
to port.

`decide_vnc` was verified against `vnc_backend_output_configure`
(main.c:3795-3841): defaults, section-only scale (C passes
`parsed_scale = 0`), forced-normal transform, `resizeable` default
true — all match, and the CLI-scale exclusion is unit-pinned.

## Findings — correctness

### PR22-C1 (minor): xkb-init failure reports the wrong error

`compositor.rs` maps a failed
`weston_compositor_set_xkb_rule_names` to
`CompositorError::CompositorCreate`, whose display text is
"weston_compositor_create failed" — a user whose xkb context failed
(bad XKB env, broken rules install) gets pointed at compositor
creation. The teardown on that path is correct (including the strdup
frees, see positives); only the variant is borrowed. A dedicated
variant ("initializing keyboard configuration failed") costs three
lines.

### PR22-C2 (nit divergence): VNC configure-failure and success log lines don't match C's

C's VNC path logs `Cannot configure output "%s" using
weston_vnc_output_api.` on failure (main.c:3834) and the odd but real
`vnc_backend_output_configure.. Done` on success (3838); the Rust
`enable_head` uses the shared windowed wording for the failure and
logs nothing on success. Nobody greps these today (the e2e suite
matches other markers), but log parity is this migration's stated
oracle discipline and the divergence is unrecorded.

### PR22-C3 (nit divergence, improvement): `multi_backend` from list length, not comma presence

C: `multi_backend = backends && strchr(backends, ',')` — so
`--backend=headless,` (trailing comma, one real backend) sets the flag
in C but not in Rust (`len() > 1`, after empty-segment filtering).
Rust's is the correct semantics; noting it only because the charter is
to record divergences, and this one is invisible until someone diffs
behavior with malformed input.

### PR22-C4 (nit): `[vnc] address` is a re-spec extension — documented in the model, absent from config-migration

The model comment says "the section spelling is a re-spec addition for
CLI/file symmetry" — good — but `docs/config-migration.md` (the
user-facing mapping) doesn't list it as a key with no ini ancestor.
Someone migrating backwards (TOML → ini, e.g. to reproduce on the C
oracle) would look for an ini `[vnc] address` that doesn't exist.

## Behavioral divergences — deliberate & documented (sound)

* The parsed-options clobbering bug not replicated (above): CLI
  geometry applies to every backend. Documented with the full C
  mechanism.
* Signal sources installed before backend load (earlier than C's
  post-display-create point is *not* the divergence — C is also
  pre-backend; the Rust ordering is now materially equivalent).
* `[vnc] address` file spelling (C4).
* The valgrind suppression file's breadth is *stated honestly* in its
  own header ("the rule below is not limited to those five blocks"),
  with the argument for why it still gates our code at zero — the
  right way to ship a suppression.

## Findings — style

### PR22-S1: `build()` is accreting inline phases

The xkb-strdup block, repeat-rate/vt-switching writes,
repaint-window validation, and `multi_backend` write now sit as ~70
inline lines of `build()` between compositor creation and backend
load. Each phase is well-commented, but `build()` is on its way to
being the new `main()` — a `fn apply_compositor_config(&ctx, &kb, …)`
would keep the bring-up sequence readable as a sequence. (R2c-color
and R2f later add more phases; the trend is visible from here.)

### PR22-S2: the three flag tables are data pretending to be code

`headless_only_flags` / `vnc_only_flags` / `unconsumed_flags` in
`reject_unported` transcribe C's per-backend `weston_option` tables —
correct now (and #27 restructured them into one row-per-flag table
when `--scale` became shared, exactly the maintenance hazard this
shape invites). The eventual shape (flag → set of consuming backends)
was already the natural one here.

## Positive notes

* The xkb ownership dance is exactly right and documented: libweston
  *stores* the passed pointers and frees them at destroy, so the
  builder strdups C-owned copies — and frees them itself on the one
  path where `set_xkb_rule_names` fails before taking ownership
  (a leak the review round caught).
* `rust-smoke.sh` leg 7's comment — "The socket is bound BEFORE the
  heads flush, so it proves only that the backend loaded" — is the
  first in-tree acknowledgment of the PR16-C6 ordering trap, and the
  fix (wait for `Output 'vnc' enabled`) puts the VNC configure path
  actually inside the valgrind window.
* Running the full e2e suite (VNC framebuffer tests included) against
  `westonite-rs` closes the observability gap that let PR19-C1 ship:
  from this PR on, "the Rust frontend has a background" is a tested
  claim.
* `KeyboardConfig` reached the model because the VNC backend
  *segfaulted* without xkb names (vnc.c:1260 strdup) — and the fix
  ported the whole `weston_compositor_init_config` slice rather than
  just papering the crash, picking up `repaint-window` (a genuine R-G
  gap) along the way.

## Cross-references

* Closes PR19-C3 (backend variable), part of PR19-C4
  (`repaint-window`, `--address`), and the observability half of
  PR19-C1's lesson.
* PR22-S2's table shape is restructured by #27 as predicted.
* C1/C2/C4 checked against current `main`: the borrowed
  `CompositorCreate` variant and generic VNC configure wording are
  still present (minor, unlisted in open items); `[vnc] address`
  remains undocumented in config-migration.md.
