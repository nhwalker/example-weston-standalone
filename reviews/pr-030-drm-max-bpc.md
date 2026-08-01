# Review: PR #30 — fix(drm): stop refusing max-bpc, which R2c-drm actually ported

| | |
|---|---|
| **PR** | [#30](https://github.com/nhwalker/example-weston-standalone/pull/30) |
| **Merge commit** | `b88b320` (base `9688d6e`) — merged 2026-07-31 |
| **Diff size** | 3 files, +38 / −2 |

## What it does and whether it's right

`reject_unported` still listed `max-bpc` in the
"color-management/DRM attributes are not yet ported" fatal block, even
though #29's `decide_drm` reads it (including the `mode=current` → 0
interaction). The fix moves it out of that block into its own
conditional refusal — fatal only when DRM is *not* loaded ("max-bpc
applies to the drm backend only"), mirroring the `clone-of` shape —
with an e2e test. Correct, minimal, and the new refusal keeps the
fail-loud property for the non-DRM case rather than making the key
silently inert there.

## Findings

### PR30-C1 (observation, systemic): the refusal table drifted from the ported surface within hours of #29 merging

`reject_unported` is a hand-maintained negative image of what's
ported; #29 grew the ported surface and the mirror lagged one PR. This
is the third instance of the pattern (see PR19-C4's piecemeal closure
list), and the eventual answer was the mechanical
key-by-key audit at #36 — which also found the *last* stale refusal
("the catch-all 'backend not yet ported' filter had been dead since
R2c-drm"). No action beyond what #36 did; recorded so the pattern has
a name: **every PR that ports a key must grep `reject_unported` for
it**, and the audit is the backstop, not the process.

No other findings — the diff is exactly as small as it should be.
