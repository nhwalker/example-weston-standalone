# Review: PR #29 — R2c-drm: the DRM backend, layoutput machinery and all

| | |
|---|---|
| **PR** | [#29](https://github.com/nhwalker/example-weston-standalone/pull/29) "R2c-drm: the DRM backend, layoutput machinery and all" |
| **Merge commit** | `9688d6e` (base `7d0af09`, head `4399382`) |
| **Merged** | 2026-07-31 |
| **Diff size** | 14 files, +874 / −34 |
| **C baseline** | `frontend/main.c`: `drm_heads_changed` (3006), `drm_head_prepare_enable` (2764), `drm_head_should_force_enable` (2797), `drm_head_disable` (2985), `drm_try_attach/enable` (2815/2836), `drm_process_layoutput(s)` (2894/2958), `wet_layoutput_*` (2647-2740) |
| **Reviewed** | `layoutput.rs` and the DRM heads-changed branch line-verified against every C counterpart above, including the undo-walk hole semantics, name reuse, and the require-outputs verdict |

## What the PR does

Lands the layoutput machinery — the reason DRM cannot ride the simple
path: heads are *staged* on named groups, then attached and enabled
together with a detach-one-and-retry undo loop, losers pushed to the
next round. Plus: the DRM heads-changed branch (connected/forced ×
enabled matrix with C's wording), `clone-of` section resolution
(retiring the R2b refusal), `require-outputs` (any/all/none),
`drm_backend_output_configure` via the DRM output API (mode /
max-bpc / scale / transform / gbm-format / content-type / seat), and
the head-detach → last-head → output-destroy path. Verified live on
vkms in the VM: 7/7, with the modeline test self-proving about which
binary ran (the Rust leg gets TOML the C frontend cannot parse).

## Verdict

The mechanically hard parts are right. I verified against C:
`try_attach` skipping index 0 (the creation head); the undo walk's
hole-preservation semantics (C decrements `n` over NULLs and clears
slots; the Rust clone-then-pop formulation reaches the same survivor
set — and the module doc explains why `add` keeps holes at all); the
`n + 1 >= ARRAY_LENGTH` cap kept one below the array size *because*
diverging would desync from the oracle in the hardest case to
reproduce; the `<group>:<head>` naming fallback with reuse only when
no output owns the plain name; the independent-CRTC clone-mode
refusal; and the `require-outputs=any` empty-list quirk (`0 == 0` ⇒
`mode=off` on the only head is a startup failure), which C leaves
implicit and this port documents *and* tests. The two non-obvious
configure defaults (`mode=current` + no `max-bpc` → 0, not 16;
single-head outputs inherit the head's transform) are preserved with
their rationales at the field.

One real fidelity break survived all of it — invisible to the vkms
harness by construction.

## Findings — correctness

### PR29-C1 (moderate divergence, claims parity it doesn't have): non-desktop DRM head handling is inverted relative to C in both directions — **fixed in #39**

C `drm_head_prepare_enable` (main.c:2764-2796), verified:

```c
section = drm_config_find_controlling_output_section(...);
if (section) {
    if (mode == "off")            return;         // skip
    if (!mode && non_desktop)     return;         // skip
    add_head(section_name, ...);                  // stage
} else {
    add_head(head_name, ...);                     // stage — no non-desktop check
}
```

So in C: a non-desktop head **with no section is staged**; a
non-desktop head **with a section that has no `mode` key is skipped**;
a section with a (non-off) `mode` stages it. The Rust `decide_drm`:

* no rule + non-desktop → `DrmHeadPlan::NonDesktop` (**skipped** —
  the comment says "C … the backend turns it off (main.c:2780)", but
  2780 is the *has-section-without-mode* branch; the check was
  transplanted to the wrong arm);
* rule present and not `off` → `Enable` unconditionally — the
  `!mode && non_desktop` skip is **missing**, so a section that merely
  sets `scale=` on an HMD stages it where C would leave it off.

Both directions were verified present on `main` at review time
(since fixed by #39). Impact
analysis: vkms heads are never non-desktop, so the VM suite cannot see
this; it becomes live exactly on the real-hardware validation that is
the declared R3 gate (an HMD/VR panel is the standard non-desktop
case). The first direction arguably implements the C comment's
*intent* ("non-desktop and not explicitly enabled" ⇒ skip) rather than
its code — a defensible divergence *if declared*, like the `bool**`
and mirror-assert precedents; the second direction is wrong by both
the code and the intent. Recommended: implement C's letter for both
branches (oracle parity), or document the intent-based behavior and
fix the missing has-section skip — either way the `main.c:2780`
citation needs correcting.

### PR29-C2 (nit): `process_one` drops C's `assert(output->output->enabled)` on existing outputs — **fixed in #49**

C asserts every surviving `wet_output` in the existing-outputs loop is
enabled before re-attaching to it; the Rust loop just uses the live
pointer. The invariant genuinely holds (an entry only lands in
`outputs` after `try_attach_enable` succeeded, and death nulls the
cell) — but the C assert is documentation of that invariant at the
point of use, and this port has been diligent about keeping exactly
such things (`debug_assert` would be the idiomatic spelling).

### PR29-C3 (nit): `decide_drm`'s unreachable-fallback arm papers over a would-be logic error — **fixed in #49**

`process_one` re-derives the group's setup via
`decide_drm(&section_head, false, …)` and on any non-`Enable` plan
falls back to `drm_defaults()` with a comment "Unreachable: a group
only exists because a head reached Enable." Passing
`non_desktop: false` hard-codes the assumption that head-level
non-desktopness can't matter at group level — true today, but this is
the same function whose non-desktop semantics C1 shows are already
subtle. A `debug_assert!` on the unreachable arm (the pattern #28
established for the DRM routing) would turn a future violation into a
loud signal instead of a silently default-configured output.

## Behavioral divergences — documented (sound)

* `require-outputs=any` + empty layoutput list ⇒ startup failure —
  C's implicit consequence, made explicit, e2e-tested.
* `[libinput]` with DRM loaded refused fail-loud (C wires
  `configure_device`; binding libinput is its own slice — landed #34/
  #35). Conditional on DRM because the section is inert elsewhere in C
  too.
* `BadCloneOf` (dangling `clone-of=`) logs and treats the head as
  sectionless — C's behavior, with the message centralized in the
  policy.

## Positive notes

* The module-level doc of `layoutput.rs` states the *shape* (stage →
  attach-then-enable-with-undo → losers to the next round) before any
  function — the single best module intro in the codebase.
* `LayoutputOutput`'s destroy listener clearing a shared
  `Rc<Cell<ptr>>` mirrors C's null-the-pointer protocol with the
  reason ("the output can be destroyed behind our back") stated.
* The modeline e2e test is self-proving about which binary ran — the
  Rust leg's TOML config is unparseable by the C frontend, so the
  1280x720 result can only have come from the port.
* `DrmRuntime` captured at builder time so the heads-changed A3 proof
  stays "wrapper state only" — consistent with the R2b pattern.

## Cross-references

* PR29-C1 **fixed in #39** (C's letter restored for both branches,
  comments corrected, unit matrices added — the only possible guard,
  since vkms never reports non-desktop heads; the R3 real-hardware
  pass remains the end-to-end confirmation).
* The max-bpc refusal interaction is #30's subject.
* `[libinput]`/`configure_device` lands in #34/#35.
