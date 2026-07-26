//! Compositor bring-up, run loop, and teardown (plan §7 R0).
//!
//! Mirrors the C frontend's headless path (`main.c`
//! `load_headless_backend` → `simple_heads_changed` →
//! `simple_head_enable`) closely enough that the Phase-1 smoke test is
//! reproducible from Rust: create display + log context + compositor,
//! load the headless backend with the noop renderer, create a head,
//! configure+enable an output per connected head, run, exit 0 on
//! SIGTERM.  The real frontend port replaces the canned output
//! configurator at R2; the primitives this file exercises are the R0
//! deliverable.

use std::ffi::{CString, c_int, c_void};
use std::ptr::NonNull;

use crate::ctx::Ctx;
use crate::events::Event;
use crate::listener::Listener;
use crate::log;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    Headless,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RendererKind {
    Auto,
    Noop,
    Pixman,
    Gl,
}

impl RendererKind {
    fn to_c(self) -> weston_sys::weston_renderer_type::Type {
        match self {
            RendererKind::Auto => weston_sys::weston_renderer_type::WESTON_RENDERER_AUTO,
            RendererKind::Noop => weston_sys::weston_renderer_type::WESTON_RENDERER_NOOP,
            RendererKind::Pixman => weston_sys::weston_renderer_type::WESTON_RENDERER_PIXMAN,
            RendererKind::Gl => weston_sys::weston_renderer_type::WESTON_RENDERER_GL,
        }
    }
}

#[derive(Debug)]
pub enum CompositorError {
    DisplayCreate,
    LogCtxCreate,
    CompositorCreate,
    BackendLoad,
    WindowedOutputApi,
    HeadCreate,
    SocketAdd,
    SignalSource,
}

impl std::fmt::Display for CompositorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            CompositorError::DisplayCreate => "wl_display_create failed",
            CompositorError::LogCtxCreate => "weston_log_ctx_create failed",
            CompositorError::CompositorCreate => "weston_compositor_create failed",
            CompositorError::BackendLoad => "loading the backend failed",
            CompositorError::WindowedOutputApi => "windowed output API unavailable",
            CompositorError::HeadCreate => "creating the head failed",
            CompositorError::SocketAdd => "wl_display_add_socket_auto failed",
            CompositorError::SignalSource => "installing signal sources failed",
        };
        f.write_str(s)
    }
}

impl std::error::Error for CompositorError {}

pub struct CompositorBuilder {
    backend: BackendKind,
    renderer: RendererKind,
    output_width: i32,
    output_height: i32,
    socket: bool,
}

impl CompositorBuilder {
    pub fn headless() -> CompositorBuilder {
        CompositorBuilder {
            backend: BackendKind::Headless,
            renderer: RendererKind::Noop,
            output_width: 1024,
            output_height: 768,
            socket: false,
        }
    }

    pub fn renderer(mut self, r: RendererKind) -> Self {
        self.renderer = r;
        self
    }

    pub fn output_size(mut self, width: i32, height: i32) -> Self {
        self.output_width = width;
        self.output_height = height;
        self
    }

    /// Also bind a wayland socket (auto name) so clients can connect.
    pub fn with_socket(mut self) -> Self {
        self.socket = true;
        self
    }

    pub fn build(self) -> Result<Compositor, CompositorError> {
        log::install_stderr_handlers();
        let ctx = Ctx::new();

        // SAFETY: plain constructor calls; null-checked before use.
        let display = unsafe { weston_sys::wl_display_create() };
        let Some(display) = NonNull::new(display) else {
            ctx.teardown();
            return Err(CompositorError::DisplayCreate);
        };
        ctx.inner.display.set(display.as_ptr());

        // SAFETY: constructor; owned by Compositor, destroyed in Drop
        // after the weston_compositor (main.c teardown order).
        let log_ctx = unsafe { weston_sys::weston_log_ctx_create() };
        if log_ctx.is_null() {
            // SAFETY: display was just created and has no clients.
            unsafe { weston_sys::wl_display_destroy(display.as_ptr()) };
            ctx.teardown();
            return Err(CompositorError::LogCtxCreate);
        }

        // SAFETY: display and log_ctx are live; user_data stays null in
        // the pure-Rust binary (the C frontend owns that slot only
        // during the hybrid phases — plan §7 R1 hazard note).
        let compositor = unsafe {
            weston_sys::weston_compositor_create(
                display.as_ptr(),
                log_ctx,
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        };
        if compositor.is_null() {
            // SAFETY: reverse creation order; nothing else refers to them.
            unsafe {
                weston_sys::weston_log_ctx_destroy(log_ctx);
                weston_sys::wl_display_destroy(display.as_ptr());
            }
            ctx.teardown();
            return Err(CompositorError::CompositorCreate);
        }
        ctx.inner.compositor.set(compositor);

        let mut comp = Compositor {
            ctx: ctx.clone(),
            log_ctx,
            heads_changed: None,
            signal_sources: Vec::new(),
            exit_code: 0,
        };

        // Sync-tier heads-changed listener (§3e names this tier: outputs
        // must be configured inside the flush).  It touches only wrapper
        // state + the canned configurator — no app borrow (A3).
        let (w, h) = (self.output_width, self.output_height);
        let listener = Listener::new(
            "heads_changed",
            false,
            Box::new(move |ctx, _data| heads_changed(ctx, w, h)),
        );
        // SAFETY: the compositor outlives the listener (Compositor owns
        // both; drop detaches before destroy).
        unsafe {
            weston_sys::weston_compositor_add_heads_changed_listener(
                compositor,
                listener.raw_ptr(),
            );
        }
        listener.mark_attached();
        comp.heads_changed = Some(listener);

        match self.backend {
            BackendKind::Headless => comp.load_headless(self.renderer)?,
        }

        // main.c:4660 — installs the (no-op) color manager and finalizes
        // backend setup; weston_output_init dereferences the color
        // manager, so this MUST precede the heads flush.
        // SAFETY: compositor live, backends just loaded.
        let ok = unsafe { weston_sys::weston_compositor_backends_loaded(compositor) };
        if ok < 0 {
            return Err(CompositorError::BackendLoad);
        }

        if self.socket {
            // SAFETY: display is live; the returned name is a borrowed
            // C string we only log (copied at the fence, §3h).
            let name = unsafe { weston_sys::wl_display_add_socket_auto(display.as_ptr()) };
            if name.is_null() {
                return Err(CompositorError::SocketAdd);
            }
            // SAFETY: non-null return from add_socket_auto is a valid
            // NUL-terminated string owned by the display.
            let name = unsafe { std::ffi::CStr::from_ptr(name) }.to_string_lossy();
            crate::log::log_line(&format!("westonite-r0: socket {name}"));
        }

        // The C frontend flushes heads once after backend setup
        // (main.c:4671); our sync-tier listener enables outputs here.
        ctx.with_depth(|| {
            // SAFETY: compositor is live.
            unsafe { weston_sys::weston_compositor_flush_heads_changed(compositor) };
        });

        // SAFETY: compositor is live.
        unsafe { weston_sys::weston_compositor_wake(compositor) };

        Ok(comp)
    }
}

pub struct Compositor {
    ctx: Ctx,
    log_ctx: *mut weston_sys::weston_log_context,
    heads_changed: Option<Listener>,
    signal_sources: Vec<*mut weston_sys::wl_event_source>,
    exit_code: i32,
}

impl Compositor {
    pub fn ctx(&self) -> &Ctx {
        &self.ctx
    }

    fn load_headless(&mut self, renderer: RendererKind) -> Result<(), CompositorError> {
        let compositor = self.ctx.inner.compositor.get();

        let mut config: weston_sys::weston_headless_backend_config =
            // SAFETY: POD config struct; zeroed then fully initialized.
            unsafe { std::mem::zeroed() };
        config.base.struct_version = weston_sys::WESTON_HEADLESS_BACKEND_CONFIG_VERSION;
        config.base.struct_size = std::mem::size_of::<weston_sys::weston_headless_backend_config>();
        config.renderer = renderer.to_c();
        config.refresh = -1;

        // SAFETY: compositor live; config outlives the call (the backend
        // copies what it needs — same contract the C frontend relies on).
        let backend = self.ctx.with_depth(|| unsafe {
            weston_sys::weston_compositor_load_backend(
                compositor,
                weston_sys::weston_compositor_backend::WESTON_BACKEND_HEADLESS,
                &mut config.base,
            )
        });
        if backend.is_null() {
            return Err(CompositorError::BackendLoad);
        }

        // weston_windowed_output_get_api is a static inline (§3k): call
        // the underlying plugin lookup directly with the installed
        // header's API name/version.
        // SAFETY: compositor live; name/version match the installed
        // header's WESTON_WINDOWED_OUTPUT_API_NAME_HEADLESS contract.
        let api = unsafe {
            weston_sys::weston_plugin_api_get(
                compositor,
                c"weston_windowed_output_api_headless_v2".as_ptr(),
                std::mem::size_of::<weston_sys::weston_windowed_output_api>(),
            )
        }
        .cast::<weston_sys::weston_windowed_output_api>();
        if api.is_null() {
            return Err(CompositorError::WindowedOutputApi);
        }

        // SAFETY: api verified non-null; create_head is set by the
        // headless backend; the name is a static C string.
        let ret = self.ctx.with_depth(|| unsafe {
            ((*api).create_head.expect("headless create_head"))(backend, c"headless".as_ptr())
        });
        if ret < 0 {
            return Err(CompositorError::HeadCreate);
        }
        Ok(())
    }

    /// Run until terminated (SIGTERM/SIGINT).  Returns the process exit
    /// code (0 on clean signal-driven shutdown — the Phase-1 contract).
    pub fn run(&mut self) -> i32 {
        let display = self.ctx.inner.display.get();
        // SAFETY: display live; loop borrowed for source installation.
        let ev_loop = unsafe { weston_sys::wl_display_get_event_loop(display) };

        // Signal-driven termination, as main.c installs for SIGTERM/INT.
        // SAFETY: display stays valid while the loop runs; the sources
        // are destroyed with the display.
        let s1 = unsafe {
            weston_sys::wl_event_loop_add_signal(
                ev_loop,
                libc::SIGTERM,
                Some(on_term_signal),
                display.cast(),
            )
        };
        // SAFETY: as s1 — same display, same loop.
        let s2 = unsafe {
            weston_sys::wl_event_loop_add_signal(
                ev_loop,
                libc::SIGINT,
                Some(on_term_signal),
                display.cast(),
            )
        };
        if s1.is_null() || s2.is_null() {
            log::log_line("westonite-r0: failed to install signal sources");
            return 1;
        }
        // Removed in Drop, as main.c removes its signals[] sources.
        self.signal_sources.push(s1);
        self.signal_sources.push(s2);

        self.ctx.with_depth(|| {
            // SAFETY: the run loop dispatches into our trampolines; every
            // one re-enters through Ctx::current + with_depth.
            unsafe { weston_sys::wl_display_run(display) };
        });
        self.exit_code
    }
}

impl Drop for Compositor {
    fn drop(&mut self) {
        // Teardown order mirrors main.c's `out:` path; shutting_down
        // first so destroy-storm policy events are discarded (§3e).
        self.ctx.inner.shutting_down.set(true);
        if let Some(l) = self.heads_changed.take() {
            l.detach();
        }
        for src in self.signal_sources.drain(..) {
            // SAFETY: sources created on this display's loop and still
            // owned by us; removed before the display dies (main.c order).
            unsafe { weston_sys::wl_event_source_remove(src) };
        }
        let compositor = self.ctx.inner.compositor.get();
        let display = self.ctx.inner.display.get();
        if !compositor.is_null() {
            self.ctx.with_depth(|| {
                // SAFETY: created in build(); destroy emits the
                // destroy storms our trampolines handle above.
                unsafe { weston_sys::weston_compositor_destroy(compositor) };
            });
        }
        self.ctx.teardown();
        // SAFETY: after compositor destroy; order per main.c.
        unsafe {
            if !self.log_ctx.is_null() {
                weston_sys::weston_log_ctx_destroy(self.log_ctx);
            }
            if !display.is_null() {
                weston_sys::wl_display_destroy(display);
            }
        }
    }
}

extern "C" fn on_term_signal(_signal: c_int, data: *mut c_void) -> c_int {
    // Async-safe enough: wl_event_loop signal sources deliver via
    // signalfd on the main loop, not in async signal context.
    // SAFETY: data is the live wl_display registered above.
    unsafe { weston_sys::wl_display_terminate(data.cast()) };
    1
}

/// The registry side of head/output tracking (§3b): registration and
/// destroy-listener attachment in one place, invoked from the sync-tier
/// heads-changed handler.
fn heads_changed(ctx: &Ctx, width: i32, height: i32) {
    let compositor = ctx.inner.compositor.get();
    if compositor.is_null() {
        return;
    }

    // §3l: snapshot the C list before doing anything that can mutate it.
    let mut heads: Vec<NonNull<weston_sys::weston_head>> = Vec::new();
    let mut iter: *mut weston_sys::weston_head = std::ptr::null_mut();
    loop {
        // SAFETY: compositor live; iterate_heads is the supported
        // enumeration API and tolerates a null cursor.
        iter = unsafe { weston_sys::weston_compositor_iterate_heads(compositor, iter) };
        match NonNull::new(iter) {
            Some(h) => heads.push(h),
            None => break,
        }
    }

    for head in heads {
        // SAFETY: head snapshot entries stay valid inside the flush; the
        // is_* getters are pure queries.
        let (connected, enabled, non_desktop) = unsafe {
            (
                weston_sys::weston_head_is_connected(head.as_ptr()),
                weston_sys::weston_head_is_enabled(head.as_ptr()),
                weston_sys::weston_head_is_non_desktop(head.as_ptr()),
            )
        };
        if connected && !enabled && !non_desktop {
            enable_head(ctx, head, width, height);
        }
    }
}

fn head_name(head: NonNull<weston_sys::weston_head>) -> String {
    // SAFETY: head live; name copied at the fence (§3h).
    unsafe {
        let p = weston_sys::weston_head_get_name(head.as_ptr());
        if p.is_null() {
            String::new()
        } else {
            std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned()
        }
    }
}

fn enable_head(ctx: &Ctx, head: NonNull<weston_sys::weston_head>, width: i32, height: i32) {
    let compositor = ctx.inner.compositor.get();
    let name = head_name(head);
    let cname = CString::new(name.clone()).unwrap_or_else(|_| c"head".into());

    // SAFETY: compositor + head live (inside the flush).
    let output = unsafe {
        weston_sys::weston_compositor_create_output(compositor, head.as_ptr(), cname.as_ptr())
    };
    let Some(output) = NonNull::new(output) else {
        crate::log::log_line(&format!(
            "westonite-r0: cannot create output for head {name}"
        ));
        return;
    };

    // Register head + output with their destroy listeners — insertion
    // and invalidator attachment paired (§3b).
    register_head(ctx, head, &name);
    register_output(ctx, output, &name);

    // SAFETY: output live and not yet enabled; the canned configurator
    // mirrors simple_head_enable + wet_configure_windowed_output_from_
    // config with fixed defaults (scale 1, normal transform, WxH).
    // (main.c's weston_output_lazy_align is frontend-local placement
    // policy; the single-output R0 smoke keeps the default (0,0) — the
    // R2 output-management port brings the real logic.)
    let enabled = unsafe {
        weston_sys::weston_output_set_scale(output.as_ptr(), 1);
        weston_sys::weston_output_set_transform(
            output.as_ptr(),
            weston_sys::wl_output_transform::WL_OUTPUT_TRANSFORM_NORMAL,
        );
        let api = weston_sys::weston_plugin_api_get(
            compositor,
            c"weston_windowed_output_api_headless_v2".as_ptr(),
            std::mem::size_of::<weston_sys::weston_windowed_output_api>(),
        )
        .cast::<weston_sys::weston_windowed_output_api>();
        !api.is_null()
            && ((*api).output_set_size.expect("output_set_size"))(output.as_ptr(), width, height)
                >= 0
            && weston_sys::weston_output_enable(output.as_ptr()) == 0
    };
    if !enabled {
        crate::log::log_line(&format!("westonite-r0: enabling output {name} failed"));
        // SAFETY: output was created above and never enabled; destroy
        // emits our destroy listener, which unregisters it.
        unsafe { weston_sys::weston_output_destroy(output.as_ptr()) };
    }
}

fn register_head(ctx: &Ctx, head: NonNull<weston_sys::weston_head>, name: &str) {
    if ctx.inner.heads.borrow().id_of(head).is_some() {
        return;
    }
    let id = crate::ids::HeadId(ctx.inner.heads.borrow_mut().insert(head));
    let name = name.to_owned();
    let key = head.as_ptr() as usize;
    let l = Listener::new(
        "head_destroy",
        true,
        Box::new(move |ctx, data| {
            // Sync half: invalidate the registry entry (§3e).
            if let Some(p) = NonNull::new(data.cast::<weston_sys::weston_head>()) {
                ctx.inner.heads.borrow_mut().invalidate_ptr(p);
            }
            // Migrate our own listener box out of the live map — its
            // trampoline frame is on the stack right now, so the box
            // rides the pending-drop list (§3f).
            let mine = {
                let mut ls = ctx.inner.listeners.borrow_mut();
                ls.iter()
                    .position(|(k, _)| *k == key)
                    .map(|i| ls.swap_remove(i).1)
            };
            if let Some(l) = mine {
                ctx.defer_drop(Box::new(l));
            }
            // Deferred half: the policy event, payload captured eagerly.
            ctx.enqueue(Event::HeadGone {
                head: id,
                name: name.clone(),
            });
        }),
    );
    // SAFETY: head is live; its death is announced through this very
    // listener (oneshot destroy pattern).
    unsafe {
        weston_sys::weston_head_add_destroy_listener(head.as_ptr(), l.raw_ptr());
    }
    l.mark_attached();
    ctx.inner.listeners.borrow_mut().push((key, l));
}

fn register_output(ctx: &Ctx, output: NonNull<weston_sys::weston_output>, name: &str) {
    if ctx.inner.outputs.borrow().id_of(output).is_some() {
        return;
    }
    let id = crate::ids::OutputId(ctx.inner.outputs.borrow_mut().insert(output));
    let name = name.to_owned();
    let key = output.as_ptr() as usize;
    let l = Listener::new(
        "output_destroy",
        true,
        Box::new(move |ctx, data| {
            if let Some(p) = NonNull::new(data.cast::<weston_sys::weston_output>()) {
                ctx.inner.outputs.borrow_mut().invalidate_ptr(p);
            }
            let mine = {
                let mut ls = ctx.inner.listeners.borrow_mut();
                ls.iter()
                    .position(|(k, _)| *k == key)
                    .map(|i| ls.swap_remove(i).1)
            };
            if let Some(l) = mine {
                ctx.defer_drop(Box::new(l));
            }
            ctx.enqueue(Event::OutputGone {
                output: id,
                name: name.clone(),
            });
        }),
    );
    // SAFETY: output is live; weston_output_add_destroy_listener hooks
    // the user-perspective destroy signal (user_destroy_signal — the
    // header-verified death announcement, §3a).
    unsafe {
        weston_sys::weston_output_add_destroy_listener(output.as_ptr(), l.raw_ptr());
    }
    l.mark_attached();
    ctx.inner.listeners.borrow_mut().push((key, l));
}

thread_local! {
    /// Track-surface refcounts, keyed by registry id (several seats may
    /// focus — and therefore track — the same surface).  There is one
    /// destroy listener per tracked surface regardless of count.
    /// Entries are removed on untrack-to-zero and on surface death; the
    /// fresh-insert path *resets* its entry to 1 (rather than
    /// incrementing), so entries orphaned by a teardown with live
    /// tracked surfaces can never bleed a stale count into a later Ctx
    /// whose table re-mints the same id shape.
    static TRACK_COUNTS: std::cell::RefCell<std::collections::HashMap<crate::ids::RawId, u32>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// Track a weston_surface for keyboard-focus bookkeeping
/// (C focus_state_set_focus's destroy listener, centralized §3b).
/// Ref-counted: several seats may focus the same surface; each
/// `track_surface` acquisition must be balanced by one
/// [`untrack_surface`] release (the C analogue is one focus_state
/// destroy listener per seat — here one shared listener plus a count).
pub(crate) fn track_surface(
    ctx: &Ctx,
    es: NonNull<weston_sys::weston_surface>,
) -> crate::ids::SurfaceId {
    if let Some(existing) = ctx.inner.surfaces.borrow().id_of(es) {
        TRACK_COUNTS.with(|m| {
            *m.borrow_mut().entry(existing).or_insert(0) += 1;
        });
        return crate::ids::SurfaceId(existing);
    }
    let id = crate::ids::SurfaceId(ctx.inner.surfaces.borrow_mut().insert(es));
    TRACK_COUNTS.with(|m| {
        m.borrow_mut().insert(id.0, 1);
    });
    let key = es.as_ptr() as usize;
    let l = Listener::new(
        "shell.tracked_surface_destroy",
        true,
        Box::new(move |ctx, data| {
            let Some(p) = NonNull::new(data.cast::<weston_sys::weston_surface>()) else {
                return;
            };
            // Eager payload: the main surface's desktop id, while the
            // object is still readable (§3e destroy split).
            // SAFETY: surface still live during its destroy emission.
            let main = unsafe {
                let m = weston_sys::weston_surface_get_main_surface(p.as_ptr());
                if m.is_null() || m == p.as_ptr() {
                    None
                } else {
                    NonNull::new(m).and_then(|m| ctx.ds_id_of_wsurf(m))
                }
            };
            TRACK_COUNTS.with(|m| {
                m.borrow_mut().remove(&id.0);
            });
            ctx.inner.surfaces.borrow_mut().invalidate_ptr(p);
            ctx.retire_listener(p.as_ptr() as usize);
            ctx.enqueue(Event::TrackedSurfaceGone { surface: id, main });
        }),
    );
    // SAFETY: surface live; its destroy_signal announces death through
    // this listener (§3b pairing).
    unsafe {
        weston_sys::wsys_wl_signal_add(&raw mut (*es.as_ptr()).destroy_signal, l.raw_ptr());
    }
    l.mark_attached();
    ctx.own_listener(key, l);
    id
}

/// Release one [`track_surface`] acquisition.  Only when the last
/// holder releases does the registry entry stale and the destroy
/// listener retire — a seat still focused on the surface keeps its
/// `TrackedSurfaceGone` delivery.
pub(crate) fn untrack_surface(ctx: &Ctx, id: crate::ids::SurfaceId) {
    let Some(p) = ctx.inner.surfaces.borrow().resolve(id.0) else {
        return;
    };
    let last = TRACK_COUNTS.with(|m| {
        let mut m = m.borrow_mut();
        match m.get_mut(&id.0) {
            Some(c) if *c > 1 => {
                *c -= 1;
                false
            }
            // Count 1 — or a missing entry, which would be a wrapper
            // bookkeeping bug: treat as last release so the listener
            // cannot leak.
            _ => {
                m.remove(&id.0);
                true
            }
        }
    });
    if last {
        ctx.inner.surfaces.borrow_mut().invalidate_ptr(p);
        ctx.retire_listener(p.as_ptr() as usize);
    }
}
