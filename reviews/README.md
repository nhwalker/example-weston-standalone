# Rust-port code reviews

A retrospective, PR-by-PR deep review of the Rust migration
(PRs #16–#36), covering three axes per PR:

1. **Correctness** — bugs, unsoundness, broken invariants;
2. **Style / understandability** — anything that makes the code harder
   to read or maintain than it should be;
3. **Behavioral divergence from the original C** — event sequencing,
   error paths, config semantics — classified *documented/intentional*
   vs. *undocumented/accidental*.

Ground rules used throughout:

* Each PR is reviewed **as of its merge commit**; findings that a later
  PR resolved are kept but annotated "fixed in #M", so still-live issues
  are distinguishable from historical ones.
* The C baseline is weston **14.0.1** (`61f2248de`) plus this repo's
  local patches P0–P4 and trims T1–T9 (see `VENDOR.md`).
* Finding IDs are `PRnn-Cx` (correctness), `PRnn-Sx` (style).
  Severities: **blocker** > **major** > **moderate** > **minor** > nit.
* Unmerged PRs (#12, #21) are not reviewed; #21's content landed via
  #20's review-fix round and is covered there.

## Status

| PR | Title (abridged) | Review | Findings (C / S) |
|---|---|---|---|
| [#16](https://github.com/nhwalker/example-weston-standalone/pull/16) | R0: workspace + weston-sys bindings | [pr-016](pr-016-r0-workspace-weston-sys.md) | 11 / 6 — incl. 1 major fixed in #26, 1 major fixed in #20 |
| [#17](https://github.com/nhwalker/example-weston-standalone/pull/17) | R1: desktop shell in Rust (hybrid) | [pr-017](pr-017-r1-rust-shell.md) | 9 / 5 — high-fidelity port; fixes two real C defects; findings mostly at doc/tier seams |
| [#18](https://github.com/nhwalker/example-weston-standalone/pull/18) | Post-merge review fixes (#11, #13–#16) | [pr-018](pr-018-post-merge-review-fixes.md) | 4 / 1 — high-quality remedial round; RPM-ships-C-shell catch + verified pointer-destroy guard |
| [#19](https://github.com/nhwalker/example-weston-standalone/pull/19) | R2a: frontend core — config, spawn, headless | [pr-019](pr-019-r2a-frontend-core.md) | 11 / 2 — 1 major latent (no background curtain under westonite-rs, fixed #20); config-surface holes closed piecemeal through #36 |
| [#20](https://github.com/nhwalker/example-weston-standalone/pull/20) | R2b: output policy (headless slice) | [pr-020](pr-020-r2b-output-policy.md) | 3 / 0 (nits) — best-executed slice; closes PR16-C2/C3, PR19-C1/C5; covers unmerged #21 |
| [#22](https://github.com/nhwalker/example-weston-standalone/pull/22) | R2c: VNC backend + multi-backend | [pr-022](pr-022-r2c-vnc-multibackend.md) | 4 / 2 — strong slice; multi_backend + sigmask catches; closes PR19-C3; first framebuffer validation |
| [#23](https://github.com/nhwalker/example-weston-standalone/pull/23) | R2c-mirror: mirror-of machinery | [pr-023](pr-023-r2c-mirror.md) | 6 / 0 — faithful port incl. the C scale quirk; exemplary handling of the C headless-source abort |
| [#24](https://github.com/nhwalker/example-weston-standalone/pull/24) | R2d: xwayland | [pr-024](pr-024-r2d-xwayland.md) | 3 / 0 — cleanest FFI file; fixes 3 C fd-hygiene defects (documented); drain gap discovered here but deferred to #26 |
| [#25](https://github.com/nhwalker/example-weston-standalone/pull/25) | R2e: screenshooter + wcap recorder | [pr-025](pr-025-r2e-screenshooter.md) | 4 / 0 — faithful; fixes C's no-outputs wild pointer; drain-gap leak measured here, fixed #26 |
| [#26](https://github.com/nhwalker/example-weston-standalone/pull/26) | Deferred-drain fix | [pr-026](pr-026-deferred-drain.md) | 3 / 0 — the right fix, red-first proof, exemplary entry-point audit; closes PR16-C1 |
| [#27](https://github.com/nhwalker/example-weston-standalone/pull/27) | R2c-nested: x11 / wayland / pipewire | *pending* | |
| [#28](https://github.com/nhwalker/example-weston-standalone/pull/28) | DRM CI probe / VM harness | *pending* | |
| [#29](https://github.com/nhwalker/example-weston-standalone/pull/29) | R2c-drm: DRM backend + layoutput | *pending* | |
| [#30](https://github.com/nhwalker/example-weston-standalone/pull/30) | fix(drm): max-bpc refusal | *pending* | |
| [#31](https://github.com/nhwalker/example-weston-standalone/pull/31) | Drop remoting plugin | *pending* | |
| [#32](https://github.com/nhwalker/example-weston-standalone/pull/32) | Drop pipewire virtual-output plugin | *pending* | |
| [#33](https://github.com/nhwalker/example-weston-standalone/pull/33) | R2c-color: colour management | *pending* | |
| [#34](https://github.com/nhwalker/example-weston-standalone/pull/34) | R2c-input 1/2: libinput/libevdev bindings | *pending* | |
| [#35](https://github.com/nhwalker/example-weston-standalone/pull/35) | R2c-input 2/2 + R2f: logging stack | *pending* | |
| [#36](https://github.com/nhwalker/example-weston-standalone/pull/36) | Close the port surface | *pending* | |

## Cross-PR open items (running list)

Issues found in one PR that were **not** fixed by any later PR in the
reviewed range — i.e. still live on `main`:

* **Signal-handling divergence** (PR16-C4): SIGINT caught via signalfd
  (C uses `sigaction`→`raise(SIGUSR2)` so gdb Ctrl+C works), no SIGUSR2
  termination route, no `caught signal %d` log line. Verified still
  present on current main.
* **Bring-up ordering** (PR16-C6): socket bound before the heads flush;
  `weston_compositor_wake` earlier than C's. *(To re-verify once #19/#22
  reviews establish the final bring-up shape.)*
* **Silent A3-violation fallback** (PR17-C1): `dispatch_sync`'s comment
  promises a loud debug-build fallback when a sync event arrives with
  the app borrow held; no assert/log exists. Verified still absent on
  current main.
* **Callback-inventory tier drift** (PR17-C2): L27 (`seat_created`) and
  L28 (`output_create`) are marked *deferred* in the inventory while
  the implementation dispatches both sync. Verified still wrong on
  current main.
* **Color parsing base mismatch** (PR19-C6): C's
  `weston_config_section_get_color` is always base-16 (unprefixed
  `ff002244` works); the Rust `parse_color` treats unprefixed values
  as decimal. Not mentioned in `docs/config-migration.md`.
