# Review: PR #32 — Drop the pipewire virtual-output plugin too

| | |
|---|---|
| **PR** | [#32](https://github.com/nhwalker/example-weston-standalone/pull/32) |
| **Merge commit** | `0305b63` (base `2ec0096`) — merged 2026-07-31 |
| **Diff size** | 5 files, +60 / −19 |

## What it does and whether it's right

Extends #31's decision to the pipewire virtual-output *plugin*, on the
same grounds. The confusable-names problem is handled head-on: weston
has two independent build options — `backend-pipewire`
("screencasting via PipeWire"), which westonite **ports and ships**,
versus `pipewire` ("Virtual remote output with Pipewire on DRM
backend"), the plugin, which it drops — and the distinction is stated
in the code comment, the refusal message itself ("the pipewire
*backend* (--backend=pipewire) is supported"), and the PROVENANCE
entry.

## Findings

None on correctness. One thing worth calling out as a **model test**:

* `test_pipewire_backend_is_still_supported` asserts the *other side*
  of the line, so the drop cannot quietly widen into the backend — and
  its docstring explains why it deliberately does **not** skip when
  the pipewire daemon is absent: the property under test is the
  frontend's verdict, and reaching a backend-load failure already
  proves the refusal didn't fire. Negative-space tests like this are
  what keep product decisions from creeping; the RDP drop (#27) got
  the same treatment. This is the pattern every future "drop X but
  keep adjacent Y" change should copy.

* Nit, carried from PR31-C2: the kept-on-purpose model field still
  lacks its own why-comment.
