# Review: PR #23 — rust(R2c-mirror): mirror-of machinery, e2e-verified via VNC-over-headless

| | |
|---|---|
| **PR** | [#23](https://github.com/nhwalker/example-weston-standalone/pull/23) "rust(R2c-mirror): mirror-of machinery, e2e-verified via VNC-over-headless" |
| **Merge commit** | `39b942c` (base `ac30317`, head `1d4e1c0`) |
| **Merged** | 2026-07-27 |
| **Diff size** | 13 files, +574 / −39 |
| **C baseline** | `frontend/main.c`: `wet_output_handle_create` (2604), `wet_output_overlap_pre/post_enable` (2530/2565), `wet_output_compute_output_from_mirror` (2538), `simple_heads_output_sharing_resize` (2581), the `simple_head_enable` mirror deferral (2021-2026) |
| **Reviewed** | full diff; every mirror handler line-verified against its C counterpart, including the modeline math and the scale-argument quirk |

## What the PR does

Ports the three C mirror pieces: the remote-head deferral in
`heads_changed` (a VNC head with `mirror-of=` waits for its source),
`mirror_on_output_created` (L6 — the source's creation enables the
deferred remote, nested inside the emission as in C), and
`mirror_on_output_resized` (L2 — reposition + recompute the remote's
modeline; one compositor-wide listener with the rule table as
discriminator, replacing C's per-tracker listener). `enable_head`
gains the `mirror_of` parameter (pre-enable position-over-source,
forced `resizeable=false` with C's log line, post-enable
source-derived modeline). Frontend validation unblocks `mirror-of`
with three startup rules; the RDP CLI drift is fixed in passing.

## Verdict

Another disciplined slice. The C machinery is reproduced with its
quirks intact — including the one most reviewers would "fix" without
noticing: `wet_output_compute_output_from_mirror` divides the source's
native mode by the *remote's* scale but passes the *source's*
`current_scale` as the new native scale (`main.c:2538-2554`); the Rust
port preserves exactly that asymmetry and documents it, so the mirror
geometry matches C bit-for-bit. The PR's own review round caught two
real fidelity gaps (the missing `weston_head_reset_device_changed`
after the nested enable — with an e2e test asserting the *absence* of
the spurious monitor-change log — and the hardcoded-VNC-configurator
error, fixed to dispatch on the head's own backend where C
`assert(wb)`s). And the ownership bug the r0 valgrind gate caught
(listeners on compositor-embedded signal lists parked in
`ctx.own_listener`, whose teardown runs after
`weston_compositor_destroy` when no shell is attached) produced both a
fix — `Compositor`-owned `frontend_listeners`, detached in `Drop`
before the destroy — and a comment explaining *why* the ownership rule
exists. That's the R2c-mirror pattern this whole series wants to see.

## Findings — correctness

### PR23-C1 (documented divergence, verified): headless mirror source aborts C; the port fails loudly instead

C's `wet_output_compute_output_from_mirror` asserts
`output->native_mode_copy.width` — and the headless backend never
populates `native_mode_copy` (it points `current_mode` at its one
static mode; drm/x11/pipewire call `weston_output_copy_native_mode`,
vnc/rdp get it via `weston_output_set_single_mode`). So the exact
configuration this slice's e2e uses (VNC mirroring headless) kills the
C oracle on an assert — reproduced live per the PROVENANCE entry, and
consistent with my reading of the C. The Rust fallback (a static
output's current mode *is* its native mode; `init_failed` when neither
exists) is the right call, the tests are honestly `toml_only` with the
abort cited, and the divergence is documented in three places. This is
the model for handling a C crash you decline to inherit.

### PR23-C2 (nit): `remote_scale.max(1)` is an undocumented safety deviation — **fixed in #49**

`apply_mirror_modeline` clamps the remote's `current_scale` to ≥1
before dividing; C divides raw (`main.c:2547-2551`). A zero scale is
unreachable (libweston asserts on scale 0 at set-scale time), so the
clamp can never fire — but it sits inside a function whose doc comment
meticulously lists its other deviations, and this one is missing from
the list. Add it or drop it.

### PR23-C3 (documented divergence): the resize path logs the modeline line C only logs at post-enable

Called out in the code comment ("a resize gains one informational line
here"). Fine — recorded for the divergence ledger since log output is
this project's oracle surface.

### PR23-C4 (documented divergence): no source-destroy → mirror-disable for simple backends

C tears the mirror down when its source output dies — but only in the
DRM layoutput path (`wet_output_handle_destroy`, main.c:2509-2527,
which I verified is layoutput-only). Simple backends have no
equivalent in C either, so the port adds none. Unreachable today
(a headless source only dies at teardown); becomes relevant if a
DRM-sourced mirror ever rides the simple path. The PROVENANCE entry
records the in-source verification — good.

### PR23-C5 (minor): mirror validation hardcodes the head name `"vnc"`

`reject_unported`: `if name != "vnc"` — correct today (the VNC backend
creates exactly one head with that name) and the comment explains the
derivation, but the *backend* owns that name, not the frontend. If VNC
ever grows multi-head (or a head-name option), this validation
silently forbids valid configs. Deriving "is a remote head's section"
from the policy/backend list rather than a string literal would be
future-proof; at minimum the string deserves a named constant shared
with wherever else "vnc" is spelled.

### PR23-C6 (nit): `mirror_on_output_resized` re-resolves by section scan on every resize

Each source resize walks the rules, scans all heads by name, and
re-derives the remote output. C's per-tracker listener has the remote
pinned. The Rust shape is simpler and the cost is negligible (resize
is rare); noting only that the `mirror_of` back-pointer check
(`(*remote).mirror_of != resized → return`) — an extra guard C doesn't
have — is doing quiet load-bearing work here and deserves its own
sentence in the comment rather than sharing one with the position set.

## Positive notes

* The **deferral branch keeps the per-head `device_changed` reset
  running** for skipped mirror heads (C's flow reaches the same reset
  through fall-through) — the sort of detail that silently diverges in
  most ports.
* The **`enabled`-head guard in L6** (skip where C would double-create
  an output) is a robustness divergence *and it's documented as one*,
  rather than slipped in as an obvious improvement.
* The RDP CLI drift fix (invented `--no-clients-resize` →
  C's `--no-resizeable`; `--rdp4-key`/`--env-socket`/
  `--no-remotefx-codec` added; `[rdp] resizeable` following the C
  field name) closes another slice of PR19-C4 — done here because it
  was *found* here, with the flags parked in the unconsumed table until
  a loader exists.
* e2e asserts the negotiated RFB framebuffer geometry (1024x640 at the
  source's size over a real connection), not just log lines — and the
  vnc-first ordering test pins the deferral specifically.

## Cross-references

* Closes the RDP-flag slice of PR19-C4.
* The `frontend_listeners` ownership rule interacts with PR17-C6's
  thread-local critique: this is the same class of lifetime problem,
  solved here by giving the boxes an owner with the right Drop order —
  the pattern `SEAT_RECS`/`TRACK_COUNTS` could follow.
* C2 and C5 checked against current `main`: both still present
  (`.max(1)` undocumented; `"vnc"` literal in the mirror validation).
