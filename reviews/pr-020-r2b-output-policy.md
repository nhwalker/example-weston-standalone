# Review: PR #20 — rust(R2b): policy-driven output management — headless slice

| | |
|---|---|
| **PR** | [#20](https://github.com/nhwalker/example-weston-standalone/pull/20) "rust(R2b): policy-driven output management — headless slice" |
| **Merge commit** | `b65f4f1` (base `3e65c95`, head `40b83e5`) |
| **Merged** | 2026-07-26 |
| **Diff size** | 15 files, +890 / −134 |
| **C baseline** | `frontend/main.c`: `simple_heads_changed` (2099), `simple_head_enable/disable` (2007/2072), `weston_output_lazy_align` (1990), `wet_configure_windowed_output_from_config`, transforms[] (1239) |
| **Reviewed** | full diff; the heads-changed branch structure, log wording, lazy_align, and transform grammar line-verified against `main.c` |
| **Note** | PR #21 (closed unmerged) was the external review of this slice; its findings were folded into this PR's "Review fixes" round before merge, so #21 is covered here and not reviewed separately. |

## What the PR does

Replaces the R0 canned output configurator with the real thing: a
plain-data `OutputPolicy` (typed `OutputTransform` matching C's
`transforms[]` grammar exactly, per-head `[[output]]` rules, CLI
override layer) resolved once by the frontend; the heads-changed
handler now implements all three C `simple_heads_changed` branches
(enable / disable-on-unplug / device-changed log) with C's exact log
wording, plus `weston_output_lazy_align`, `--no-outputs`,
`--refresh-rate`, `--transform`, and an `off` re-spec extension. The
review round folded in the `init_failed` port and the
registration-after-enable fix.

## Verdict

This is the best-executed slice so far, and its review round fixed
three findings this series traced from earlier PRs — each with an
explanatory comment good enough that the *reason* is now in the code:

* **`init_failed` ported** (closes PR16-C2): `Ctx` carries the flag,
  `build()` checks it right after the flush exactly where C checks
  (`main.c:4672`), and the three failure paths use C's wording — with
  the nice observation that the enable-failure line is deliberately
  *not* duplicated because libweston already logs the identical text.
* **Registration moved after `weston_output_enable`** (closes PR19-C1
  and PR16-C3): the comment at the new call site spells out both
  consequences of the old order (swallowed `Event::OutputCreated`, two
  destroy listeners on one address). With the shell attached, the
  shell's `output_created` listener now registers the output first and
  the frontend's `register_output` takes the idempotent early-return —
  one registration, one destroy listener, curtain restored.
* **Heads iteration filtered by backend** (the C
  `wet_backend_iterate_heads` semantics): a `Cell<*mut weston_backend>`
  discriminator on `Ctx`, with the comment explaining what would break
  the moment a second backend loads. (R2c-vnc later generalized this
  to a per-backend list, as anticipated.)

Branch structure, log strings ("Detected a monitor change on head
'%s', not bothering to do anything about it."), the
`weston_head_reset_device_changed` per-head reset, and the
`transforms[]` name set were all verified against `main.c` — no
deviations found beyond those listed below.

## Findings — correctness

### PR20-C1 (nit divergence): `lazy_align` keeps `f64` where C truncates through `int` — **fixed in #48**

C: `int next_x = peer->pos.c.x + peer->width;` — the peer's double x
is truncated to int before reuse (`main.c:1994-2003`). Rust:
`next_x = (*peer).pos.c.x + f64::from(width)` keeps the fraction. The
two differ only when a peer output sits at a fractional x, which no
current frontend path produces — but this function exists precisely
for the multi-output future, and the divergence is the kind that shows
up as a one-pixel seam years later. Either truncate to match C or
leave a comment stating the deliberate improvement. **#48 took the
first option**: C's truncation restored (with the residual
`as`-saturation difference documented at the cast), pinned by unit
tests over hand-wired list structs.

### PR20-C2 (nit): `parse_mode`'s positivity check is a third undocumented strictness — **fixed in #49**

The doc comment enumerates two shapes the C `sscanf` waves through
that Rust rejects (embedded spaces, trailing junk) — but the `w > 0 &&
h > 0` guard is a third: C would pass `-100x480` straight to
`output_set_size`. It's the right call (same rationale as the
`--width` validation); it just belongs in the same comment's list.

### PR20-C3 (nit): the `off`-rule head is registered with a destroy listener that can outlive its purpose

The once-per-head "disabled by config" log gate reuses
`register_head`'s return value, which also attaches a head-destroy
listener and mints a `HeadId` for a head that will never have an
output. Harmless (the listener just invalidates on head death), but
"register = log gate" is a side-effect coupling: someone deleting the
registration later (say, when off-heads get their own tracking) also
silently deletes the log's once-only property.

## Behavioral divergences from C — deliberate, and properly documented

The PR's own documentation of divergences is the standard the other
slices should be held to. For the record:

* **All `[[output]]` sections validated eagerly** (C resolves a
  section only when a matching head appears, so typo'd names and
  unparseable transforms in unmatched sections are silently inert).
  Rationale documented at `build_output_policy`; bad transform is
  fatal with C's `Invalid transform "…"` wording.
* **No "Invalid mode" warning for a section without a `mode` key** —
  C logs "Invalid mode for output %s. Using defaults." even when the
  section merely sets `scale` (`!mode || sscanf(…)` in
  parse_simple_mode). Documented as deliberate noise reduction.
* **`off = true` / `mode = "off"` for a windowed head** — a re-spec
  extension (C windowed backends have no per-output off switch);
  logged once per head.
* A section without `name=` is fatal (C: inert forever).

## Positive notes

* The `lazy_align` doc comment proves the subtle invariant (the new
  output is on `pending_output_list`, not `output_list`, so it is
  never its own placement peer) instead of just asserting it — and
  honestly flags that the peer branch is unreachable until R2c brings
  a second backend, rather than claiming test coverage it doesn't
  have.
* `OutputPolicy` as plain data keeps the heads-changed trampoline's A3
  proof trivially true ("pure table lookup") — the design pattern the
  plan wanted, realized cleanly.
* The `[[output]] scale` positivity check moved into
  `westonite-config` *next to the CLI checks it shares a rule with*,
  with an error message naming the section — small, but exactly where
  a maintainer would look.
* e2e coverage grew in the right dimensions: `--transform` from CLI,
  bad transform fatal, invalid-mode fallback, `--no-outputs`, and the
  Rust-only `off` semantics, each against the real binary.
* PR16-C6 note: with the socket now bound after the shell attach but
  before the flush, the smoke scripts' "socket exists ⇒ outputs up"
  inference is still wrong — unchanged here, and it bit at R2c-vnc
  (leg 7 waited on the socket; #22 fixed the wait to
  `Output 'vnc' enabled`).

## Cross-references

* Closes PR16-C2 (init_failed), PR16-C3 / PR19-C1 (registration after
  enable), part of PR19-C4 (`--transform` gap), PR19-C5 (parse_mode
  trim).
* The single-backend `Cell` discriminator is superseded by #22's
  per-backend list, as its own comment predicts.
* C1 (lazy_align truncation) checked against current `main`: the f64
  arithmetic is still there (`compositor.rs`), now reachable via
  multi-backend layouts — remains a live nit.
