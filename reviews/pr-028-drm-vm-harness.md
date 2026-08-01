# Review: PR #28 — ci(drm): probe round 3 → the DRM VM harness

| | |
|---|---|
| **PR** | [#28](https://github.com/nhwalker/example-weston-standalone/pull/28) "ci(drm): probe round 3 — does the C frontend drive vkms in CI?" |
| **Merge commit** | `7d0af09` (base `55ea480`, head `a17be0f`) |
| **Merged** | 2026-07-31 |
| **Diff size** | 15 files, +1,224 / −62 |
| **Reviewed** | VM harness (Containerfile.drm-vm, drm-vm-init.sh, drm-vm-test.sh), CI restructuring, and the preparatory DRM Rust plumbing (loader, `DrmMode`/`DrmOutputSetup`/`decide_drm`), against `load_drm_backend` and `drm_backend_output_configure` in `main.c` |

## What the PR does

Despite the title ("probe round 3"), what merged is the complete DRM
test environment plus preparatory Rust plumbing: a VM carrying its own
CentOS Stream 10 kernel inside the build image (`/dev/kvm` optional,
TCG fallback), `vkms` as the KMS device, seatd, a results disk read
back without privileges, `tests/e2e/test_backend_drm.py` (7 tests,
gated on `WESTONITE_DRM_VM=1`), and — ahead of #29 — `load_drm`, the
`DrmOptions` struct, and the typed `DrmMode`/`DrmOutputSetup`/
`decide_drm` policy layer. The frontend still refuses
`--backend=drm`, so the new Rust code is unreachable except through
the builder API. The throwaway CI probes are deleted, their findings
preserved in `docs/drm-testing.md` §1.

## Verdict

As test infrastructure this is exceptional work, and the *recorded
failures* are its best feature: the two probe mistakes worth keeping
(vkms registering on the **faux bus** since ~6.14, so
driver-name matching finds nothing; `LIBSEAT_BACKEND=builtin` not
existing in EL10's libseat) are written down where the next person
will look, and the four dead-end probe rounds were deleted from CI
instead of rotting. Three design decisions deserve singling out:

* **The sentinel goes out through `/dev/kmsg`, and the reasoning is a
  war story**: the first KVM-accelerated run reported "no sentinel"
  for a fully passing suite because a `/dev/console` write is buffered
  by the tty layer and the sysrq power-off printk landed mid-string —
  a race only KVM was fast enough to hit. printk emits records whole.
  The verdict comes from the sentinel, not qemu's exit status (qemu
  exits 0 for a panicked guest), which is the correct trust model.
* **No privileges anywhere**: `mkfs.ext4 -d` builds the rootfs from a
  directory without mounting; `debugfs -R dump` reads the JUnit XML
  back out. No loop devices, no CAP_SYS_ADMIN. This is the difference
  between a harness that runs on any runner and one that works until
  the CI provider changes a default.
* **Gating on `WESTONITE_DRM_VM=1` and *not* on `/dev/dri`** — a
  developer's workstation has a real DRM device, and taking DRM master
  on it would black out their display. The guard is documented with
  that reason.

The shell scripting shows the same care (the `if` vs.
`[ -c /dev/kvm ] && ACCEL=kvm` note — a bare false test under
`set -e` would kill the script instead of selecting TCG — and the
real-copy staging comment explaining how hardlinks would poison the
baked rootfs).

## Findings

### PR28-C1 (nit): dangling `crate::layoutput` doc links at this commit

`DrmOptions`' and `load_drm`'s doc comments reference
``[`crate::layoutput`]`` — a module that does not exist until #29.
Intra-doc link resolution is a rustdoc lint, and CI has no `cargo doc`
leg, so nothing catches it (then or now). Harmless here because #29
landed hours later; the general gap — broken doc links are invisible
to this CI — might be worth a nightly `cargo doc -D warnings` leg
given how load-bearing this codebase's doc comments are.

### PR28-C2 (nit, history): the PR title undersells the diff

"probe round 3 — does the C frontend drive vkms in CI?" merged 1,224
lines including the whole VM harness and the DRM loader groundwork.
The PROVENANCE entries (R2c-drm-harness) tell the real story, but
anyone navigating by PR title will misfile this one. Squash-merge
titles are cheap to fix at merge time.

### PR28-C3 (nit): dead-until-#29 DRM plumbing rides in with a well-designed guard

`head_setup_for`'s `BackendKind::Drm` arm is
`debug_assert!(false, "DRM heads go through the layoutput path")` with
a comment noting the release path degrades to the visible
"disabled by config" log rather than UB — verified accurate (the
`None` return takes the off-rule branch). Shipping unreachable
plumbing one PR early is a reasonable slicing choice; the guard makes
it safe. No action — recorded because "unreachable" code with a loud
tripwire is the right pattern and worth naming as such.

## Positive notes

* `decide_drm`'s two preserved C subtleties are documented **at the
  field**: `mode=current` with no explicit `max-bpc` passes 0 rather
  than 16 (so reusing the current mode doesn't force a full modeset),
  and `transform: None` means "substitute the head's own transform,
  which this layer cannot see" — both verified against
  `drm_backend_output_configure` (main.c:2333-2378).
* The raw `mode_string` kept alongside the parsed size, because only
  the backend can resolve a modeline — the frontend deliberately does
  not over-model (§5), and says so.
* `WESTONITE_E2E_TIMEOUT_SCALE` (set to 4 only under TCG) instead of
  loosening every deadline — slow-path tolerance without hiding
  fast-path startup regressions.
* The DRM outputs-advertise-every-mode finding (34 modes on vkms; the
  active one is the `flags: current` entry, not the first `width:`
  line) came from a red test and is now harness knowledge.
* PID-1 bring-up facts (no inherited environment, so `PATH` before the
  first `modprobe`; results disk mounted by **label** because no udev
  means no `/dev/disk/by-label`) are recorded in the init script where
  they're load-bearing.

## Cross-references

* The harness is consumed by #29 (R2c-drm) — the checkpoint that
  justified the route (`Output 'Virtual-1' enabled` under the C
  frontend on vkms) is this PR's exit criterion.
* PR28-C1's missing `cargo doc` leg is a standing CI gap, not specific
  to this PR.
