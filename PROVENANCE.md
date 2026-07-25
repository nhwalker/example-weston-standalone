# Rust migration provenance (plan §8)

Maps every Rust module that translates C behavior to its source C file
at the upstream tag current at translation time (`14.0.1` + P0).  This
file replaces VENDOR.md's verbatim-import model for translated code;
VENDOR.md continues to govern the C sources until R3 removes them.
Rebase procedure on an EPEL weston bump: plan §8.

| Rust module | Source C @ tag | Notes |
|---|---|---|
| `crates/weston/src/compositor.rs` (bring-up, heads-changed, teardown) | `frontend/main.c` @ 14.0.1: `load_headless_backend` (3472), `simple_heads_changed` (2099), `simple_head_enable` (2007), signal/teardown paths (`main`, `out:`) | R0 **canned** mirror for the smoke exit criterion only; the R2a/R2b frontend port supersedes it and this row then points at the real modules |
| `crates/weston/src/listener.rs` | idiom port: `wl_listener`/`container_of` pattern (34 sites) | design, not line translation — see docs/callback-inventory.md |
| `crates/weston/src/grab.rs` | idiom port: `shell_grab` (`shell.c:117-140`, vtables at 660-925) | structure at R0; policy lands at R1 |
| `crates/weston-sys/shim/shim.c` | static inlines from wayland-util/server headers + weston-log va_list pair | §3k |

## Migration log

- **R0 (2026-07-25)** — foundation phase (plan §7 R0), branch
  `claude/rust-migration-j84b2p`:
  - Cargo workspace beside the meson tree; `weston-sys` (bindgen over
    the installed EPEL 14.0.1 headers, bindings committed per D5, regen
    via `scripts/regen-bindings.sh`, pkg-config version tripwire in
    `build.rs`, `cc` shim) and the `weston` fence crate.
  - §3 primitives: generational-id registry (§3b), `Listener` (§3c),
    two-tier dispatch with depth-counted drain (§3e/A4), `Grab`
    skeleton with deferred drop (§3f), panic barrier (D16), thread-local
    `Ctx` slot (D21), `ShellHost` core (D20), fake-C-object test
    harness (D18).
  - §3 header facts re-verified against the installed headers:
    `docs/r0-header-facts.md` (2 live findings: mandatory
    `weston_compositor_backends_loaded`, `weston_output_lazy_align` is
    frontend-local).  Callback inventory: `docs/callback-inventory.md`.
  - Exit criterion met: `r0-smoke` (compositor + headless + noop
    renderer) exits 0 on SIGTERM — clean under valgrind (0 errors,
    0 definite leaks) and AddressSanitizer (nightly `-Zsanitizer=address`).
  - CI: rustfmt / clippy `-D warnings` (+ `undocumented_unsafe_blocks`,
    `unsafe_op_in_unsafe_fn`) / unit tests / fence checks
    (`scripts/rust-fence-check.sh`) / valgrind smoke / ASAN job.
    Build image gains rust-toolset 1.96, clang-libs, valgrind.
