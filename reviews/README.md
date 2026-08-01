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
| [#27](https://github.com/nhwalker/example-weston-standalone/pull/27) | R2c-nested: x11 / wayland / pipewire | [pr-027](pr-027-r2c-nested.md) | 3 / 0 — probe-first porting; 1 live divergence (wayland default-head numbering) |
| [#28](https://github.com/nhwalker/example-weston-standalone/pull/28) | DRM CI probe / VM harness | [pr-028](pr-028-drm-vm-harness.md) | 3 / 0 (nits) — exceptional test infra; kmsg-sentinel + no-privilege design; title undersells scope |
| [#29](https://github.com/nhwalker/example-weston-standalone/pull/29) | R2c-drm: DRM backend + layoutput | [pr-029](pr-029-r2c-drm.md) | 3 / 0 — layoutput machinery verified faithful; the non-desktop inversion (PR29-C1) **fixed in #39** |
| [#30](https://github.com/nhwalker/example-weston-standalone/pull/30) | fix(drm): max-bpc refusal | [pr-030](pr-030-drm-max-bpc.md) | 1 / 0 — correct minimal fix; names the refusal-table drift pattern |
| [#31](https://github.com/nhwalker/example-weston-standalone/pull/31) | Drop remoting plugin | [pr-031](pr-031-drop-remoting.md) | 2 / 0 — honest product decision; closes the silent-no-op sections |
| [#32](https://github.com/nhwalker/example-weston-standalone/pull/32) | Drop pipewire virtual-output plugin | [pr-032](pr-032-drop-pipewire-output.md) | 0 / 0 — model negative-space test for the backend/plugin line |
| [#33](https://github.com/nhwalker/example-weston-standalone/pull/33) | R2c-color: colour management | [pr-033](pr-033-r2c-color.md) | 3 / 0 (nits) — faithful; closes final PR19-C4 items; grouping rule + refcount protocol verified |
| [#34](https://github.com/nhwalker/example-weston-standalone/pull/34) | R2c-input 1/2: libinput/libevdev bindings | [pr-034](pr-034-r2c-input-bindings.md) | 1 / 0 — minimal; the link-test pattern highlighted |
| [#35](https://github.com/nhwalker/example-weston-standalone/pull/35) | R2c-input 2/2 + R2f: logging stack | [pr-035](pr-035-libinput-config-logging.md) | 3 / 0 — punctuation-faithful libinput port; RAII LogContext; deprecated enable_tap spelling dropped undocumented |
| [#36](https://github.com/nhwalker/example-weston-standalone/pull/36) | Close the port surface | [pr-036](pr-036-close-port-surface.md) | 2 / 0 — completes the PR19-C4 audit; the mark_attached soundness bug (PR36-C1) **fixed in #38** |

## Resolved since the review

Findings fixed after the review series landed:

* **PR36-C1** (`--debug` authority listener never `mark_attached` —
  latent UAF at teardown): **fixed in #38**, which also added the
  drop-time attached assert in `Compositor::drop` and rust-smoke
  leg 9 (`--debug` under valgrind, debug build) so the bug class
  fails loudly.
* **PR29-C1** (DRM non-desktop head handling inverted vs C in both
  directions): **fixed in #39** — C's letter restored for both
  branches with corrected `main.c` citations, pinned by the
  `drm_non_desktop_matrix_matches_c` and
  `drm_non_desktop_follows_the_controlling_section` unit tests (the
  only possible guard until the real-hardware R3 validation, since
  vkms never reports non-desktop heads).
* **PR17-C1** (`dispatch_sync`'s A3-violation fallback silent despite
  the comment's "loud in debug builds" promise): **fixed in #41** —
  the loud path keys on a sync handler being on the stack outside a
  drain, so mid-drain requeueing and builder-phase dispatch stay
  quiet; pinned by three unit tests (loud / mid-drain / pre-app).
* **PR35-C1** (C's deprecated `enable_tap` spelling dropped with a
  generic unknown-field error and no migration note): **fixed in
  #43** — the key stays in the model (the `modules=` precedent) so
  the refusal names the rename, `config-migration.md` gains the row,
  unit + e2e tests pin both halves.

## Cross-PR open items (running list)

Issues found in one PR that were **not** fixed by any later PR — i.e.
still live on `main`:

* **Signal-handling divergence** (PR16-C4): SIGINT caught via signalfd
  (C uses `sigaction`→`raise(SIGUSR2)` so gdb Ctrl+C works), no SIGUSR2
  termination route, no `caught signal %d` log line. Verified still
  present on current main.
* **Bring-up ordering** (PR16-C6): final shape (post-#36) is
  backends_loaded → shell attach → socket → flush → xwayland → wake,
  vs C's flush → socket → shell → xwayland → wake. The shell-before-
  flush half is documented (with rationale) at `with_shell`; the
  socket-before-flush half is acknowledged only in a smoke-script
  comment. Live divergence, low impact.
* **Callback-inventory tier drift** (PR17-C2): L27 (`seat_created`) and
  L28 (`output_create`) are marked *deferred* in the inventory while
  the implementation dispatches both sync. Verified still wrong on
  current main.
* **Color parsing base mismatch** (PR19-C6): C's
  `weston_config_section_get_color` is always base-16 (unprefixed
  `ff002244` works); the Rust `parse_color` treats unprefixed values
  as decimal. Not mentioned in `docs/config-migration.md`.
* **Nested-wayland default head numbering** (PR27-C1): C numbers
  default heads `wayland0..` regardless of named WL-sections; the port
  numbers them after the named count (`waylandN..`). An `[[output]]`
  section targeting `wayland0` can silently match nothing. Verified
  live on main.
* **lazy_align f64 vs C int truncation** (PR20-C1): reachable now via
  multi-backend layouts; still f64 on main.
