# Review: PR #34 — R2c-input (1/2): bind libinput and libevdev

| | |
|---|---|
| **PR** | [#34](https://github.com/nhwalker/example-weston-standalone/pull/34) |
| **Merge commit** | `13de735` (base `f043c7c`) — merged 2026-08-01 |
| **Diff size** | 5 files, +1,814 / −2 (of which +1,759 generated bindings) |

## What it does

Pure bindings slice ahead of the `[libinput]` port: pkg-config probes
for libinput + libevdev in `build.rs`, the two headers in `wrapper.h`
(with the reason they enter directly — libweston only
*forward-declares* `struct libinput_device`, and the fence calls
`libinput_device_config_*` itself), regen allowlist entries, and the
committed bindings. The hand-written surface is ~55 lines.

## Verdict

Correct, minimal, and it contains one small piece of engineering worth
highlighting as a pattern: the **link test**. A bindings-only commit
can be wrong in exactly one way no compile error catches — producing
declarations the linker can't resolve — and `cargo build` doesn't
catch it either, because with `--as-needed` a library nothing
references yet is dropped from `DT_NEEDED` entirely. The
`link_tests` module takes the addresses of five representative symbols
(spanning both new libraries and the specific
`libinput_device_config_*` family the next slice needs), which forces
resolution at test-link time. The doc comment explains all of this,
including why the assert itself is almost incidental. This is the
right way to commit bindings ahead of their consumer.

## Findings

### PR34-C1 (nit): the libinput allowlist is much broader than the consumer

`--allowlist-item 'libinput_.*|LIBINPUT_.*'` pulls the entire libinput
API (event types, tablet tools, gesture events — ~1,700 generated
lines) where the port uses only the `device_config_*` family plus
`get_name`/capability queries. The libevdev entry, by contrast, is
surgically scoped (`libevdev_event_code_from_name` + `EV_KEY`). Not
wrong — the weston allowlist is equally broad by policy — but the
asymmetry between the two entries in the same diff suggests the narrow
form was considered and the broad one chosen without a recorded
reason. Generated-code bloat is cheap; drift surface in the fence
crate is less so.

No other findings.
