# Review: PR #31 — Drop the remoting plugin; make both virtual-output sections fail loudly

| | |
|---|---|
| **PR** | [#31](https://github.com/nhwalker/example-weston-standalone/pull/31) |
| **Merge commit** | `2ec0096` (base `b88b320`) — merged 2026-07-31 |
| **Diff size** | 5 files, +101 / −3 |

## What it does and whether it's right

Drops the remoting plugin (`[[remote-output]]`, GStreamer/RTP output
streaming) as a product decision, and turns both virtual-output
sections into startup errors with e2e tests. The decision record in
PROVENANCE is unusually honest: remoting is **not** deprecated
upstream (15.0.90 still builds it by default and documents
`remoting=`), the probe that preceded the decision had shown it
*working* in the VM harness — so this is a decision not to ship a
working feature, stated as such, with VNC named as the remote path.
What upstream actually deprecated is `screen-share`, which westonite
never shipped and whose replacement (mirroring) was already ported at
R2c-mirror — the distinction is spelled out so nobody later confuses
the two.

## Findings

### PR31-C1 (confirmation of PR19-C4, closed here): `[[remote-output]]` and `[[pipewire-output]]` had been silent no-ops since the model landed

Both sections parsed into the R2a config model and *nothing ever read
them* — a user asking for a remote output got silence and no output,
"the exact failure the fail-loud rule exists to prevent" (the PR's own
words). This retroactively confirms the modeled-but-unread pattern
PR19-C4 flagged (`vrr-mode`, `max-cll` were the same class). Both now
refuse with tests, "because a refusal without a test is how this
recurs" — the right epistemics. Note the intermediate state: this PR
made `[[pipewire-output]]` refuse as *not yet ported*; #32 dropped it
an hour later and reworded. The two-step is honest history rather than
retconned intent.

### PR31-C2 (nit): the model fields stay on purpose — the reason is recorded elsewhere

`remote_output`/`pipewire_output` remain in the serde model so the
refusal can say *what happened* instead of the generic unknown-field
error a removed field would give. That rationale is written in
PROVENANCE (for `modules=`, and by extension here) but not at the
model fields themselves, where a future "dead field" cleanup would
look first. One sentence on each field would inoculate them.

No other findings.
