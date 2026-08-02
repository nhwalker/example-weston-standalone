# Review: PR #35 — R2c-input (2/2) + R2f: [libinput] device config and the weston-log scope stack

| | |
|---|---|
| **PR** | [#35](https://github.com/nhwalker/example-weston-standalone/pull/35) |
| **Merge commit** | `11ef01a` (base `13de735`, head `742a05f`) — merged 2026-08-01 |
| **Diff size** | 23 files, +1,677 / −118 |
| **C baseline** | `main.c`: `configure_input_device` + `_accel`/`_scroll` (2131-2330); `vlog`/`vlog_continue` (214/241), log setup (4499-4534), teardown (4817-4823), `DEFAULT_FLIGHT_REC_*` (76-77) |
| **Reviewed** | `libinput.rs` line-verified against the three C configure functions (including log punctuation); `log.rs`'s `LogContext` design against C's setup/teardown order; harness changes |

## What it does

Two halves. **R2c-input**: `[libinput]` parsing/validation in the
frontend (`build_input_config`), application per device in
`libinput.rs` via the `weston_drm_backend_config.configure_device`
hook — DRM-only, as in C. **R2f**: the weston-log scope/subscriber
stack — the "log" scope, file subscriber, flight recorder with its
Super+D dump binding, `--logger-scopes`/`-l` and
`--flight-rec-scopes`/`-f` now functional (the last two warn-and-ignore
flags in the tree), `--wait-for-debugger` (plus the missing `[core]`
config key), and log-context ownership moved to the frontend as an
RAII `LogContext` whose drop order reproduces main.c:4817-4823
exactly.

## Verdict

Both halves are strong. On the libinput side I verified `apply` /
`apply_accel` / `apply_scroll` branch-for-branch against C — including
the details that show real care:

* **C's inconsistent log punctuation is reproduced exactly**: periods
  on `enable-tap=%s.` / `tap-and-drag=%s.` / `tap-and-drag-lock=%s.` /
  `disable-while-typing=%s.`, none on `middle-button-emulation` /
  `left-handed` / `rotation` / the accel and scroll lines. Whoever
  ports this to a newer weston will thank the faithful quirk.
* The **capability-gating boundary is drawn correctly and argued**:
  invalid *values* (unknown accel-profile, out-of-range accel-speed,
  scroll-button without scroll-method=button) are startup errors —
  unconditional, because they could work on no backend — while a
  *device* that lacks a capability is skipped in silence per C,
  because that's a runtime hardware fact. The "not a divergence"
  paragraph in PROVENANCE is as valuable as the divergence list.
* `ScrollButton` keeps the configured *name* because C echoes the name
  in its log line, not the resolved code — with a test pinning that,
  plus NUL-injection and bare-number rejection.
* The scroll-method "none is always settable, others must be
  advertised" gate matches C's
  `method != NO_SCROLL && (method & methods) == 0` precisely.
* **`disable-while-typing` was missing from the model entirely** — C
  reads it (main.c:2294) — found by this slice's own sweep; the same
  class as `allow_hdcp` (#33).

On the R2f side, the design decision is the right one and the
PROVENANCE entry documents both the *what* and the *why*: the Rust
frontend's log was never going through libweston at all (shim-formatted
straight to a file — no timestamps, closing PR19-C7), and routing
through the scope is also what makes subscribers possible at all.
`LogContext` owns ctx/scope/subscribers/`FILE*` with the C teardown
order, and the failure mode of getting it wrong is documented as
observable ("debug scope 'log' has not been destroyed" — how valgrind
caught the first attempt). The absent-vs-empty `--flight-rec-scopes`
distinction (default list vs. disabled) matches C.

## Findings

### PR35-C1 (minor divergence, undocumented, live on main): C's deprecated `enable_tap` spelling is dropped without a migration note — **fixed in #43**

C honors the underscore spelling with a `!!DEPRECATION WARNING!!` and
then applies it (main.c:2261-2271). The kebab-case model has no alias,
so an ini config using the deprecated-but-working spelling fails the
TOML migration with a generic unknown-field error — and
`docs/config-migration.md` doesn't mention it. Dropping a deprecated
key is a fine re-spec decision; the migration doc is exactly where it
needs one sentence. (This is also the only `[libinput]` key where the
generic error names a key the C docs never used — the user typed what
their old weston.ini had.) **Fixed in #43**: the key stays in the
model (the `modules=` precedent from #36) so the refusal names the
rename, `config-migration.md` gained the row, and unit + e2e tests
pin both halves.

### PR35-C2 (documented divergence, for the ledger): the no-scope logging fallback

C has no path for a log line before the context exists or after it
dies — those lines are dropped. The shim falls back to the sink (file
or stderr) so panic-barrier output after teardown still lands
somewhere. Documented in the module doc; recorded here because it
means the Rust log can contain lines a C log never would, which a
strict log-diff oracle run would flag.

### PR35-C3 (nit): `apply`'s device log line ends with a period the section lines echo inconsistently — matched to C, worth one comment

Precisely *because* the punctuation is faithfully inconsistent, a
future cleanup ("normalize log punctuation") would silently break
oracle parity. One line in `apply`'s doc comment saying the
inconsistency is load-bearing would protect it. (The module doc says
log lines mirror C; it doesn't say the periods are part of that.)

## Positive notes

* The DRM VM harness additions close a vacuous-pass hole with an
  explicit guard: udevd is required because libinput's udev backend
  matches `ID_INPUT=1` and nothing else sets it — without it the hook
  never fires and the tests pass while testing nothing, so the guest
  init **asserts a non-zero tagged-device count**. And `-vga none`
  exists because coldplug modprobed Bochs VGA and weston picked it
  over vkms — caught by mode assertions against `Virtual-2`, diagnosed
  to the root.
* `Device` as a safe view with **one shared safety argument stated
  once** and per-method pointers back to it — the cleanest way this
  codebase has yet handled a family of twenty near-identical FFI
  accessors.
* All six R2f e2e tests run against the C oracle too — this is
  libweston's own machinery wired identically on both sides, so the
  oracle passing them *is* the parity proof (the tests aren't
  `toml_only` where they didn't need to be).

## Cross-references

* Closes PR19-C7 (timestamps), the last two warn-and-ignore flags
  (PR19's documented exceptions), and adds the missing
  `[core] wait-for-debugger` model key (PR19-C4's final straggler
  before #36's audit).
* Retires the `[libinput]`-with-DRM refusal carried since #29.
* PR35-C1 added to the open-items list.
