# Review: PR #33 — R2c-color: colour management

| | |
|---|---|
| **PR** | [#33](https://github.com/nhwalker/example-weston-standalone/pull/33) |
| **Merge commit** | `f043c7c` (base `0305b63`, head `9d50f5a`) — merged 2026-08-01 |
| **Diff size** | 12 files, +766 / −36 |
| **C baseline** | `main.c`: `wet_output_set_color_profile` (1365), `_set_eotf_mode` (1403), `_set_colorimetry_mode` (1464), `_set_color_characteristics` (1483-1644), `allow_content_protection` (1679), the load site (1203), per-backend call sets (1865-1890 vs 2415-2423) |
| **Reviewed** | `apply_color` + `decide_color`/`ColorSetup` against all five C helpers; EOTF flow and error wording verified; per-backend scope (`full` flag) verified against the windowed vs DRM configure call sets |

## What it does

Ports colour management: `[core] color-management` loads the colour
manager at C's exact point (before backends, main.c:1203), and the
`[[output]]` colour keys (`icc-profile`, `eotf-mode`,
`colorimetry-mode`, `color-characteristics`, `allow-hdcp`) apply
through one fence function, `apply_color`, replacing C's four helpers
+ `allow_content_protection` — because "the only thing that differs
between backends is *which* of them get called, and that is the
caller's business." The `full` flag encodes that split (windowed/remote
configures: hdcp + profile only; DRM/headless: everything), verified
against the C call sets. Two model corrections ride along: `allow-hdcp`
was **missing** from the model entirely (C reads it on every backend —
unreachable until now), and `vrr-mode` + `max-cll` were **fictional**
(no C reader / no such struct field) and are removed, closing the last
items of PR19-C4's list.

## Verdict

Faithful where it should be, and divergent only on purpose:

* The **EOTF support check** reproduces C's wording
  (`Error: output '%s' does not support EOTF mode %s.`) and its
  ordering (name grammar validated frontend-side; hardware support at
  configure time). Verified against main.c:1403-1448.
* The **grouping rule** — the eleven characteristic keys form five
  groups (primaries, white, maxL, minL, maxFALL), each given entirely
  or not at all — is C's subtle part (the per-key group table at
  main.c:1542-1560) and the port reproduces it as a validation error
  rather than a partial application, with `group_mask()` assembling
  exactly C's mask.
* The **one deliberate divergence** is the right one and documented at
  the code: C's `wet_output_set_color_profile` returns *before
  reading* `icc_profile` when no colour manager is loaded, so
  `icc-profile` without `color-management=true` is silently ignored in
  C; the frontend refuses the combination. Consistent with the
  fail-loud doctrine and stated in PROVENANCE.
* The **icc profile refcount protocol** (load → set → unref our
  reference whether or not the set succeeded) matches C's.

## Findings

### PR33-C1 (nit, style): `apply_color`'s `full: bool` is a boolean trap

Call sites read `apply_color(ctx, output, color, false)` — the reader
must know that `false` means "windowed scope: hdcp + profile only".
The comment at one call site explains it; the other relies on it. A
two-variant enum (`ColorScope::ProfileOnly | Full`) would make every
call site self-documenting for the cost of four lines.

### PR33-C2 (nit): enum-to-name via array indexing

`EotfMode::NAMES[eotf as usize]` (and the colorimetry twin) couples
the display name table to variant declaration order with nothing
enforcing the correspondence. A `fn name(self)` match — which the
compiler exhaustiveness-checks — is the idiom the rest of the codebase
uses (`Backend::name`, `WindowedApi::name`).

### PR33-C3 (recorded, found by the PR itself): the drm-only refusal masks validation errors it precedes

The PR's own red test found that the "these keys are drm-only" refusal
runs before name/range validation, so a headless output masks the
error under test; the validation e2e cases request `--backend=drm`
purely to get past that gate (no VM needed — config validation
precedes backend load). Correct workaround, honestly documented in the
tests; the underlying ordering (backend-applicability before value
validity) is a defensible UX choice but means a user fixing errors
sees them serially, applicability first. Noted, no action.

## Positive notes

* The harness finding (`ini_to_config` typed bools and ints but not
  floats, so the first float keys arrived quoted and failed
  deserialization) was fixed in the harness, not worked around in the
  model — the translation layer stays a faithful D11 mapping.
* Removing the two fictional keys makes `deny_unknown_fields` honest:
  the model now rejects `vrr-mode` *with a span* instead of accepting
  a key libweston can never apply — the exact failure D9 exists to
  prevent, applied to the project's own earlier mistake.
* `use_color_manager` on `Ctx` keeps `apply_color`'s no-manager
  behavior decidable without re-consulting settings — and the
  invariant ("reaching here with an icc profile means the manager is
  loaded") is stated where it's relied upon.

## Cross-references

* Closes the final PR19-C4 items (`allow-hdcp` added; `vrr-mode`/
  `max-cll` removed).
* The `[libinput]`-with-DRM refusal recorded at #29 is unchanged here;
  it falls at #34/#35.
