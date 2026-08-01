# Review: PR #36 — Close the port surface: drop modules + calibrator, port --debug, output-decorations and the renderer aliases

| | |
|---|---|
| **PR** | [#36](https://github.com/nhwalker/example-weston-standalone/pull/36) |
| **Merge commit** | `20e26d8` (base `11ef01a`, head `bbb1107`) — merged 2026-08-01 |
| **Diff size** | 12 files, +763 / −79 |
| **C baseline** | `main.c`: `protocol_log_fn` (267), `get_next_argument` (246), `screenshot_allow_all` (4396), the `--debug` block (4618-4631), protologger teardown (4793/4803), `load_headless_backend`'s use-gl/use-pixman reads (3490) |
| **Reviewed** | `debug.rs` in full against C's protocol logger; the model/resolve additions; the mechanical-audit outcomes |

## What it does

The completeness audit: a mechanical diff of every
`weston_config_section_get_*` key (67) and every `WESTON_OPTION_*`
flag (48) in `main.c` against the config model and the clap struct —
the audit PR19-C4 asked for, finally executed — closing the last three
gaps (two ported, one dropped): `--debug` + the "proto"
protocol-dump scope, `[core] output-decorations`, and
`[core] use-gl`/`use-pixman` as config keys; `[libinput]`
touchscreen-calibrator dropped as a product decision;
`modules=`/third-party plugin loading dropped (reversing plan D2, with
the amendment recorded in place); the dead catch-all "backend not yet
ported" refusal removed; and a unit test that uncomments every key in
`westonite.toml.example` and parses it through the real
`deny_unknown_fields` model — verified by reintroducing a planted bug.

## Verdict

The audit methodology and its honesty are excellent (see positives),
and `debug.rs` is a careful port — the `types[]`-indexed-by-argument
C quirk preserved with its rationale, formatting Rust-side into a
length-delimited write so client-controlled text can never reach a
format string, and byte-parity verified against the oracle with
`--logger-scopes=proto`. But the PR ships one genuine soundness bug in
the fence, and it is this review's most concrete new finding.

## Findings — correctness

### PR36-C1 (moderate→major, live on main): `debug::enable` attaches its authority listener without `mark_attached()` — under `--debug`, the pinned box is freed while still linked in the compositor's signal list

`debug.rs::enable` hands `auth.raw_ptr()` to `wsys_wl_signal_add`
(onto `compositor->output_capture.ask_auth`) and returns the
`Listener` **without calling `mark_attached()`**. This violates the
primitive's own written contract — `listener.rs` (since R0): "Callers
that hand this to a C add-listener API MUST call
`Listener::mark_attached` right after, or detach-on-drop will skip the
node." The identical pattern eight lines away in
`screenshooter.rs::create` does call it.

Consequence, whenever `--debug` is passed: at `Compositor::drop`, the
`frontend_listeners` drain calls `detach()` — which sees
`attached == false` and no-ops — then drops the box, freeing the
pinned `ListenerInner` **while its `wl_listener` node is still linked
on the live compositor's `ask_auth` list**. With today's exact
teardown ordering nothing dereferences the dangling node afterwards
(the screenshooter's authority detaches earlier in the drain, and
`weston_compositor_destroy` doesn't walk `ask_auth`), so this is
latent rather than an active use-after-free — but it is one teardown
reordering, or one future ask_auth attach/detach sequenced after the
debug drop, away from list surgery through freed memory. It also
means the listener is *never actually detached*, so the
detach-before-destroy discipline the `frontend_listeners` mechanism
exists to enforce (the R2c-mirror lesson) is silently defeated for
this one node.

Fix is one line (`auth.mark_attached();` before returning — matching
`screenshooter.rs`). Verified missing on current `main`
(`debug.rs`, tail of `enable`). Worth pairing with a debug assertion
or fence-check grep: every `raw_ptr()` handed to an attach API should
be followed by `mark_attached` — the contract exists, this shows it
isn't mechanically enforced.

### PR36-C2 (nit): `format_argument`'s `h` arm prints the compositor-side fd number — same as C, worth its one-line note

`fd {}` prints `a.h`, which after libwayland's demarshalling is the
*receiving* side's fd. C does the same; noting only because the dump
line reads like the client's fd and the quirk-preservation standard
elsewhere in this file (`types[]`) is a stated comment.

## Deliberate divergences — documented (sound)

* **`use-gl`/`use-pixman` config keys apply on every backend** where
  C reads them only in `load_headless_backend` (main.c:3490), leaving
  the file spelling silently inert elsewhere while the CLI flags work
  everywhere — called an "accident of where the read sits", diverged
  from knowingly. Recorded with the C line cited.
* **`proto` scope installed unconditionally, not gated on `--debug`**
  — as in C, with the cost argument (one branch per message when
  unsubscribed) stated.
* **`modules=` dropped, reversing plan D2** — and D2 is amended *in
  place with the date and reasoning* rather than quietly rewritten;
  the config key stays in the model on purpose so the refusal names
  the product decision instead of a generic unknown-field error.
  (This also finally writes down the keep-the-field rationale that
  PR31-C2 asked for.)
* **Touch calibration dropped** — product decision with the concrete
  grounds (no calibrator client shipped, nothing to test against).

## Positive notes

* **The audit found its own tooling gap and fixed it**: the
  `westonite.toml.example` uncomment-and-parse unit test exists
  because the audit caught `wait-for-debugger` documented under the
  wrong section — and the test was validated by *reintroducing the
  bug and watching it fail*. Test-of-the-test discipline.
* Removing the dead catch-all refusal ("a guard that cannot trigger
  reads like a live one") closes the loop PR30-C1 named.
* `test_unported_feature_fails_loudly` renamed
  `test_unsupported_feature_fails_loudly` with the migration-log note
  that it has moved targets three times because "the gate is the
  *principle*, not any one feature" — the right way to keep a
  sentinel test honest as its targets are consumed.
* The `--debug` authority uses a `Listener` (not a bare fn) for the
  same helper-overwrites-notify reason as R2e, with the precedent
  cited — consistency across slices, except for the one missing line
  in C1.

## Cross-references

* Completes PR19-C4 (the mechanical audit this review kept asking
  for), and with it the last R3 blocker per the migration log.
* **PR36-C1 added to the cross-PR open-items list** — the only
  soundness-class finding currently live on `main`.
