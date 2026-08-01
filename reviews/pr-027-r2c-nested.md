# Review: PR #27 — rust(R2c-nested): x11, wayland and pipewire backends — e2e-tested, not inspected

| | |
|---|---|
| **PR** | [#27](https://github.com/nhwalker/example-weston-standalone/pull/27) "rust(R2c-nested): x11, wayland and pipewire backends — e2e-tested, not inspected" |
| **Merge commit** | `55ea480` (base `7c66114`, head `628c619`…`a146fb1` range) |
| **Merged** | 2026-07-29 |
| **Diff size** | 15 files, +852 / −59 |
| **C baseline** | `frontend/main.c`: `load_x11_backend` (3924), `load_wayland_backend` (4044), `load_pipewire_backend` (3611), their `*_output_configure` (3910/4030/3558), the section-name create_head loops (3985-4020 / 4125-4160) |
| **Reviewed** | full compositor/policy/main diffs; the head-creation loops, prefix filters, and per-backend defaults line-verified against `main.c` |

## What the PR does

Ports the three nested backends: `load_x11` / `load_wayland` /
`load_pipewire` mirroring the C loaders, a shared
`create_windowed_heads` for the section-name-then-default create_head
loop ('X'→`screenN`, "WL"→`waylandN`), `HeadSetup::Windowed` now
carrying *which* windowed plugin API to drive, `HeadSetup::Pipewire`,
and `OutputPolicy::decide_sized` for the per-backend defaults (x11's
1024x600 has its own test). The frontend's fail-loud flag tables are
restructured from per-backend lists into one row per flag with its
consuming-backend set. RDP is dropped as a product decision. Notably,
the premise changed before the code: these were slated
"inspection-verified, zero container reachability", and probing the
environment **with the C frontend first** showed all three run in the
container — so they shipped with real e2e tests instead.

## Verdict

The environment-first discipline is the headline: rather than porting
blind, the slice proved the container could host all three backends
under the C oracle (discovering along the way that EL10 ships no X.org
at all — the compositor under test supplies the X server for the
compositor under test) and then held the port to running tests. Two
latent bugs died before they could ship: `HeadSetup::Windowed`
previously hardcoded the *headless* plugin API, which would have
silently misconfigured x11/wayland outputs (now carried per-head), and
the old three-list flag tables would have rejected valid invocations
once `--scale`/`--use-pixman` became multi-backend flags (exactly the
maintenance hazard PR22-S2 predicted; the new one-row-per-flag shape
is the fix).

## Findings — correctness

### PR27-C1 (minor divergence, live on main): nested-wayland default head numbering differs from C when named and default heads mix

`create_windowed_heads` numbers default heads `made..count` for both
windowed backends. That matches C's **x11** loop
(`for (i = output_count; i < option_count; i++)` → `screen%d` with i
starting after the named heads) — but C's **wayland** loop decrements
`count` per named head and then numbers from zero
(`for (i = 0; i < count; i++)` → `wayland%d`, main.c:4152-4160). With
`--output-count=3` and one `[[output]]` section named `WL-1`, C
creates `WL-1, wayland0, wayland1`; the port creates
`WL-1, wayland1, wayland2`. Any `[[output]]` section targeting
`wayland0` then silently matches nothing (and under this project's
eager validation, targeting a name that never appears isn't even
rejected — the rule just sits inert, the exact failure mode the
fail-loud rule exists to catch). Reachable only with
`--output-count > 1` plus WL-named sections — rare, but the C
asymmetry between the two loops is precisely the kind of quirk the
shared helper flattened. Verified still `made..count` for both on
current `main`.

### PR27-C2 (nit): `output_count.max(1)` / `num_outputs.max(1)` silently repair what C would take literally

C passes `--output-count=0` through (`option_count` 0 → zero heads for
x11 — arguably useless but honored); the port clamps to 1. Defensible
— but the clamp is undocumented, and elsewhere (`--width=0`) the
project's answer to useless values is a startup error, not a silent
repair. One of the three treatments (honor, reject, clamp) should be
chosen on purpose and written down.

### PR27-C3 (nit): `create_windowed_heads` consumes named sections even when `made >= count` differs subtly from C's break placement

C checks `output_count >= option_count` *before* examining each
section; Rust filters the names first and breaks inside the loop —
same observable result for matching sections, but C also stops
*scanning* (a later section with a matching name is never validated),
while the Rust prefilter has already collected every match. No
behavioral difference today (create_head is the only effect); worth a
comment since the loop shapes look more equivalent than they are.

## Behavioral divergences — deliberate & documented (sound)

* **RDP dropped** (owner's call, recorded in-tree): `--backend=rdp`
  fails with a message pointing at VNC; its CLI flags stay parsed in
  the unconsumed table. The refusal is Rust-only policy, so its e2e
  test is honestly `toml_only`.
* All three nested backends default to the GL renderer, so the harness
  passes `--renderer=pixman` exactly as the VNC leg does — an
  environment fact, documented.
* The `westonite` e2e fixture tears down instances in **reverse**
  creation order, because a nested child exits non-zero when its host
  dies first — a teardown artifact correctly kept out of the
  pass/fail signal.

## Positive notes

* **Probe-first porting**: the environment was proven with the C
  frontend before any Rust was written, converting "inspection-only"
  backends into tested ones and shrinking the DRM unknowns to a
  disposable CI probe (`drm-vkms-probe`, `continue-on-error`, with a
  delete-me note).
* `WindowedApi` as data on `HeadSetup` — the per-backend configure
  flavor is now impossible to forget, where the previous shape made
  the headless API a silent default.
* The C loaders' string-lifetime contract ("C frees its copies right
  after the call, so temporaries are the contract") is restated at
  each loader instead of assumed — three loaders, three explicit
  SAFETY notes.
* Validation ran everything both ways: Rust 69/1, C oracle 58/12,
  every new test green against both frontends.

## Cross-references

* Closes PR22-S2 (flag-table shape).
* PR27-C1 added to the cross-PR open-items list (live on main).
* The `drm-vkms-probe` throwaway job feeds #28's harness decision.
