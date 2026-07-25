# R0 header-fact verification (plan §3a caveat)

Every §3 claim the wrapper design keys off, re-verified against the
**installed** EPEL 10 `weston-devel-14.0.1-3.el10_0` headers — the same
headers bindgen consumes — on 2026-07-25 inside the build container.
Re-run this check (and the bindings regen) whenever EPEL bumps weston
(plan §8; the pkg-config tripwire in `crates/weston-sys/build.rs` makes
a silent bump impossible).

| # | Claim (plan §3) | Verified against 14.0.1 headers | Result |
|---|---|---|---|
| F1 | `weston_keyboard` / `weston_touch` have **no** `destroy_signal` | struct definitions in `libweston.h` (only `focus_signal` + grab state) | ✅ — "never store input sub-objects" rule stands |
| F2 | `weston_seat`, `weston_surface`, `weston_view` announce death via `destroy_signal` | struct fields present | ✅ |
| F3 | `weston_pointer` has `destroy_signal` | field present | ✅ |
| F4 | `weston_output` death for *users* is `user_destroy_signal`, attached via `weston_output_add_destroy_listener`; the sibling `destroy_signal` fires **when disabled** | `libweston.h:548` (user), `:610` ("sent when disabled") | ✅ — registry must use the add/get-destroy-listener API, never the disable signal (encoded in `compositor.rs::register_output`) |
| F5 | `weston_head` has `destroy_signal` + dedicated add/get-listener API | `libweston.h:481`, `:2656-2661` | ✅ |
| F6 | Compositor destroy via `weston_compositor_add_destroy_listener_once` | `libweston.h:2451` | ✅ |
| F7 | `weston_desktop_surface` has **no** signal; death is the `surface_removed` API callback; exactly two user-data slots exist (`weston_desktop_surface_{set,get}_user_data`, get-only `weston_compositor_get_user_data`) | `desktop.h:154/187`, `libweston.h:2481` | ✅ |
| F8 | `weston_desktop_api` has 14 function-pointer entries, `struct_size`d | `desktop.h` (counted mechanically) | ✅ |
| F9 | `weston_curtain_params` has the `get_label` **callback** (15.x changed it to `char *label` — the reference-tree trap the plan flagged) | `shell-utils.h` | ✅ — 14.x shape confirmed; sync-tier, no-app-borrow row in the callback inventory |
| F10 | `weston_pointer_grab_interface`: 7 entries (focus, motion, button, axis, axis_source, frame, cancel) | `libweston.h` | ✅ — grab vtable in `grab.rs` matches |
| F11 | Headless config: `{base, renderer, decorate, refresh}`, `WESTON_HEADLESS_BACKEND_CONFIG_VERSION` | `backend-headless.h:39-52` | ✅ |
| F12 | Windowed output API v2 name `weston_windowed_output_api_headless_v2`; `weston_windowed_output_get_api` is a **static inline** wrapper over `weston_plugin_api_get` | `windowed-output-api.h:40, 89-103` | ✅ — Rust calls `weston_plugin_api_get` directly; no shim needed |
| F13 | Static-inline inventory of the installed headers: plugin-api getters (drm ×2, pipewire ×2, rdp, vnc, windowed, xwayland ×2, remoting), `matrix.h` coord constructors, `zalloc` | grep over `/usr/include/libweston-14` | ✅ — none need the C shim; getters → direct `weston_plugin_api_get`, coord math → plain Rust, `zalloc` irrelevant.  The shim carries only the wayland-util/server inlines (`wl_list_*`, `wl_signal_add`) + the va_list log pair (§3k) |
| F14 | `weston_compositor_create(display, log_ctx, user_data, test_data)` signature | `libweston.h:2443` | ✅ |
| F15 | Renderer enum: AUTO=0, NOOP=1, PIXMAN=2, GL=3 | `libweston.h:2465` | ✅ |

Findings that adjusted the R0 code (not §3 contradictions):

- **`weston_output_lazy_align` is not libweston API** — it is a static
  helper in `frontend/main.c:1990` (frontend-local placement policy).
  The R0 canned configurator skips it (single output at (0,0)); the
  R2b output-management port implements the real placement logic.
- **`weston_compositor_backends_loaded` is mandatory** before any
  output is created: it installs the default (no-op) color manager,
  and `weston_output_init` dereferences `compositor->color_manager`
  unconditionally.  Missing it is a startup segfault (found live, fixed
  in `compositor.rs`; the C frontend calls it at `main.c:4660`).
- The installed `windowed-output-api.h` does not parse standalone: its
  static inline uses weston's internal `ARRAY_LENGTH` macro, which the
  RPM does not install.  The bindings regen script supplies the macro
  (`scripts/regen-bindings.sh`).
