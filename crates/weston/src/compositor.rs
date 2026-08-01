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
use crate::output_policy::{OutputPolicy, OutputSetup};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    Headless,
    Vnc,
    X11,
    Wayland,
    Pipewire,
    Drm,
}

/// Headless backend options (C load_headless_backend's CLI slice).
#[derive(Debug, Clone, Default)]
pub struct HeadlessOptions {
    /// C --no-outputs: create no head at all.
    pub no_outputs: bool,
    /// C --refresh-rate, mHz; backend default -1.
    pub refresh_mhz: Option<i32>,
}

/// VNC backend options (C load_vnc_backend: CLI + `[vnc]` section,
/// already merged by the frontend's resolution).
#[derive(Debug, Clone, Default)]
pub struct VncOptions {
    /// C --address; backend default: all interfaces.
    pub bind_address: Option<String>,
    /// C --port / `[vnc] port`; backend default 5900.
    pub port: Option<i32>,
    /// C `[vnc] refresh-rate`, Hz; VNC_DEFAULT_FREQ (60) otherwise.
    pub refresh_rate_hz: Option<i32>,
    pub tls_cert: Option<String>,
    pub tls_key: Option<String>,
    pub disable_tls: bool,
}

#[derive(Debug, Clone)]
enum BackendSpec {
    Headless(HeadlessOptions),
    Vnc(VncOptions),
    X11(X11Options),
    Wayland(WaylandOptions),
    Pipewire(PipewireOptions),
    Drm(DrmOptions),
}

/// The two backend-level settings the DRM heads-changed branch needs
/// that are not per-output policy.
#[derive(Debug, Clone, Copy, Default)]
struct DrmRuntime {
    /// C --current-mode: every output takes its current mode.
    current_mode: bool,
    /// C `[core] require-outputs` — how hard a failed layoutput is.
    require_outputs: crate::layoutput::RequireOutputs,
}

/// DRM/KMS backend options (C load_drm_backend, main.c:3375).
///
/// Unlike every other backend this one creates no heads of its own on
/// load: the backend discovers connectors and the heads-changed
/// listener does the work, through the layoutput machinery rather than
/// `simple_heads_changed` (see [`crate::layoutput`]).
#[derive(Debug, Clone, Default)]
pub struct DrmOptions {
    /// C --seat / the libinput seat id; backend default "seat0".
    pub seat_id: Option<String>,
    /// C --drm-device, e.g. "card0"; backend default: pick one.
    pub specific_device: Option<String>,
    /// C --additional-devices.
    pub additional_devices: Option<String>,
    /// C `[core] gbm-format` — the backend-wide default, distinct from
    /// the per-output `gbm-format=`.
    pub gbm_format: Option<String>,
    /// C `[core] pageflip-timeout`, ms; 0 disables.
    pub pageflip_timeout: u32,
    /// C `[core] pixman-shadow`, default true.
    pub use_pixman_shadow: bool,
    /// C --current-mode: force every output to its current mode.
    pub current_mode: bool,
    /// C --continue-without-input: clears compositor->require_input, so
    /// a machine with no keyboard or mouse still starts.
    pub continue_without_input: bool,
}

/// X11 backend options (C load_x11_backend's CLI slice, main.c:3947).
#[derive(Debug, Clone, Default)]
pub struct X11Options {
    pub fullscreen: bool,
    pub no_input: bool,
    /// C --output-count, default 1: how many `screenN` heads to create
    /// beyond the `[[output]]` sections whose name starts with 'X'.
    pub output_count: i32,
}

/// Wayland (nested) backend options (C load_wayland_backend, 4070).
#[derive(Debug, Clone, Default)]
pub struct WaylandOptions {
    /// C --display: parent compositor socket (None = WAYLAND_DISPLAY).
    pub display_name: Option<String>,
    pub fullscreen: bool,
    /// C --sprawl: one output spanning the parent's outputs; the
    /// windowed-output API is then absent and nothing is configurable.
    pub sprawl: bool,
    pub output_count: i32,
    /// C `[shell] cursor-theme` / `cursor-size` (default 32).
    pub cursor_theme: Option<String>,
    pub cursor_size: i32,
}

/// PipeWire backend options (C load_pipewire_backend, 3625).
#[derive(Debug, Clone, Default)]
pub struct PipewireOptions {
    /// C `[core] gbm-format` (the backend-wide one).
    pub gbm_format: Option<String>,
    /// C `[pipewire] num-outputs`, default 1.
    pub num_outputs: i32,
}

/// `[keyboard]` slice of C weston_compositor_init_config, applied
/// between compositor creation and backend load.  Mandatory before any
/// backend that creates a keyboard: the VNC backend strdup's
/// `compositor->xkb_names` unconditionally (vnc.c:1260), so an
/// uninitialized set segfaults it.  `None` fields let libweston fill
/// its own defaults (evdev/pc105/us).
#[derive(Debug, Clone, Default)]
pub struct KeyboardConfig {
    pub rules: Option<String>,
    pub model: Option<String>,
    pub layout: Option<String>,
    pub variant: Option<String>,
    pub options: Option<String>,
    /// C default 40.
    pub repeat_rate: Option<i32>,
    /// C default 400.
    pub repeat_delay: Option<i32>,
    /// C default true.
    pub vt_switching: Option<bool>,
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
    ShellInit,
    OutputInit,
    Xwayland,
    ColorManager,
}

impl std::fmt::Display for CompositorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            CompositorError::DisplayCreate => "wl_display_create failed",
            CompositorError::LogCtxCreate => "weston_log_ctx_create failed",
            CompositorError::CompositorCreate => "weston_compositor_create failed",
            CompositorError::BackendLoad => "loading the backend failed",
            CompositorError::ColorManager => "loading the color manager failed",
            CompositorError::WindowedOutputApi => "windowed output API unavailable",
            CompositorError::HeadCreate => "creating the head failed",
            CompositorError::SocketAdd => "binding the wayland socket failed",
            CompositorError::SignalSource => "installing signal sources failed",
            CompositorError::ShellInit => "shell initialization failed",
            CompositorError::OutputInit => "configuring the outputs failed",
            CompositorError::Xwayland => "loading the xwayland module failed",
        };
        f.write_str(s)
    }
}

impl std::error::Error for CompositorError {}

type ShellFactory = Box<dyn FnOnce(u32) -> Box<dyn crate::ctx::ShellApp>>;

pub struct CompositorBuilder {
    backends: Vec<BackendSpec>,
    renderer: RendererKind,
    policy: OutputPolicy,
    keyboard: KeyboardConfig,
    repaint_window_msec: Option<i32>,
    socket: bool,
    socket_name: Option<String>,
    shell: Option<(u32, ShellFactory)>,
    /// Some(xserver path) loads xwayland.so in build() (C main.c:4725).
    xwayland: Option<std::path::PathBuf>,
    /// C `[core] require-outputs` — DRM only (it is the only backend
    /// whose outputs can fail to come up).
    require_outputs: crate::layoutput::RequireOutputs,
    /// C `[core] color-management` (main.c:1203).
    color_management: bool,
}

impl Default for CompositorBuilder {
    fn default() -> Self {
        CompositorBuilder::new()
    }
}

impl CompositorBuilder {
    /// No backends yet — add them in load order (C load_backends walks
    /// the comma list in order, primary first).  build() fails on an
    /// empty list.
    pub fn new() -> CompositorBuilder {
        CompositorBuilder {
            backends: Vec::new(),
            renderer: RendererKind::Noop,
            // C headless_backend_output_configure defaults.
            policy: OutputPolicy::defaults(1024, 640),
            keyboard: KeyboardConfig::default(),
            repaint_window_msec: None,
            socket: false,
            socket_name: None,
            shell: None,
            xwayland: None,
            require_outputs: crate::layoutput::RequireOutputs::default(),
            color_management: false,
        }
    }

    /// Load the xwayland module at build() and lazily spawn `xserver`
    /// on the first X connection (C --xwayland / [core] xwayland).
    pub fn with_xwayland(mut self, xserver: std::path::PathBuf) -> Self {
        self.xwayland = Some(xserver);
        self
    }

    /// `[keyboard]` configuration (C weston_compositor_init_config).
    pub fn with_keyboard(mut self, kb: KeyboardConfig) -> Self {
        self.keyboard = kb;
        self
    }

    /// `[core] repaint-window` (ms; C validates the -10..=1000 range
    /// with a warning and keeps libweston's default otherwise).
    pub fn with_repaint_window_msec(mut self, msec: i32) -> Self {
        self.repaint_window_msec = Some(msec);
        self
    }

    /// One default headless backend (the R0 smoke path).
    pub fn headless() -> CompositorBuilder {
        CompositorBuilder::new().add_headless(HeadlessOptions::default())
    }

    pub fn add_headless(mut self, opts: HeadlessOptions) -> Self {
        self.backends.push(BackendSpec::Headless(opts));
        self
    }

    pub fn add_x11(mut self, opts: X11Options) -> Self {
        self.backends.push(BackendSpec::X11(opts));
        self
    }

    pub fn add_wayland(mut self, opts: WaylandOptions) -> Self {
        self.backends.push(BackendSpec::Wayland(opts));
        self
    }

    pub fn add_pipewire(mut self, opts: PipewireOptions) -> Self {
        self.backends.push(BackendSpec::Pipewire(opts));
        self
    }

    pub fn add_vnc(mut self, opts: VncOptions) -> Self {
        self.backends.push(BackendSpec::Vnc(opts));
        self
    }

    pub fn add_drm(mut self, opts: DrmOptions) -> Self {
        self.backends.push(BackendSpec::Drm(opts));
        self
    }

    /// C `[core] require-outputs` (main.c:4644, default "any").
    pub fn require_outputs(mut self, r: crate::layoutput::RequireOutputs) -> Self {
        self.require_outputs = r;
        self
    }

    /// C `[core] color-management=true`: load the colour manager.
    /// Without it every colour key is inert in C, so the frontend
    /// refuses the combination rather than reproducing that silence.
    pub fn color_management(mut self, on: bool) -> Self {
        self.color_management = on;
        self
    }

    /// Renderer selection, applied to every backend loaded (C passes
    /// the one CLI/global choice into each loader's config.renderer).
    pub fn renderer(mut self, r: RendererKind) -> Self {
        self.renderer = r;
        self
    }

    /// Default output geometry only (R0 smoke sugar); the frontend
    /// passes a full [`OutputPolicy`] instead.
    pub fn output_size(mut self, width: i32, height: i32) -> Self {
        self.policy.default_size = (width, height);
        self
    }

    /// The resolved output configuration (R2b): per-head `[[output]]`
    /// rules + CLI overrides, consulted by the heads-changed handler.
    pub fn with_output_policy(mut self, policy: OutputPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Also bind a wayland socket (auto name) so clients can connect.
    pub fn with_socket(mut self) -> Self {
        self.socket = true;
        self
    }

    /// Bind a wayland socket with an explicit name (C --socket).
    pub fn with_socket_name(mut self, name: &str) -> Self {
        self.socket = true;
        self.socket_name = Some(name.to_string());
        self
    }

    /// Attach the built-in Rust shell (R2+: statically linked policy
    /// crate) after backend setup, before the heads flush — so the
    /// shell's output listeners see every output creation.
    #[cfg(feature = "hybrid-r1")]
    pub fn with_shell(
        mut self,
        background_color: u32,
        factory: impl FnOnce(u32) -> Box<dyn crate::ctx::ShellApp> + 'static,
    ) -> Self {
        self.shell = Some((background_color, Box::new(factory)));
        self
    }

    pub fn build(self) -> Result<Compositor, CompositorError> {
        log::install_stderr_handlers();
        // Checked before anything is created: a builder with no backend
        // can never produce a running compositor, and failing here
        // keeps the error off the teardown paths below.
        if self.backends.is_empty() {
            return Err(CompositorError::BackendLoad);
        }
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

        // C weston_compositor_init_config, between compositor create
        // and backend load: [keyboard] xkb names (mandatory for
        // keyboard-creating backends — VNC strdup's them, vnc.c:1260),
        // repeat rates, vt-switching, [core] repaint-window.
        // SAFETY: compositor live; xkb_rule_names is read by the call.
        // libweston STORES the passed pointers and frees them at
        // compositor destroy, so it gets C-owned strdup copies; nulls
        // let it fill its own defaults (evdev/pc105/us, input.c:3954).
        let xkb_ok = unsafe {
            let mut names: weston_sys::xkb_rule_names = std::mem::zeroed();
            names.rules = c_strdup_opt(self.keyboard.rules.as_deref());
            names.model = c_strdup_opt(self.keyboard.model.as_deref());
            names.layout = c_strdup_opt(self.keyboard.layout.as_deref());
            names.variant = c_strdup_opt(self.keyboard.variant.as_deref());
            names.options = c_strdup_opt(self.keyboard.options.as_deref());
            let r = weston_sys::weston_compositor_set_xkb_rule_names(compositor, &mut names);
            if r < 0 {
                // libweston bails before `ec->xkb_names = *names`
                // (input.c: only xkb_context_new can fail there), so the
                // strdup'd copies never changed owner — free them or the
                // C-owned allocations leak on this path.
                for p in [
                    names.rules,
                    names.model,
                    names.layout,
                    names.variant,
                    names.options,
                ] {
                    libc::free(p.cast_mut().cast());
                }
            }
            r >= 0
        };
        if !xkb_ok {
            // C weston_compositor_init_config returns immediately here,
            // so none of the settings below are applied or logged.
            // SAFETY: reverse creation order, same as the paths above.
            unsafe {
                weston_sys::weston_compositor_destroy(compositor);
                weston_sys::weston_log_ctx_destroy(log_ctx);
                weston_sys::wl_display_destroy(display.as_ptr());
            }
            ctx.teardown();
            return Err(CompositorError::CompositorCreate);
        }
        // SAFETY: compositor live; plain POD field writes on it.
        unsafe {
            (*compositor).kb_repeat_rate = self.keyboard.repeat_rate.unwrap_or(40);
            (*compositor).kb_repeat_delay = self.keyboard.repeat_delay.unwrap_or(400);
            (*compositor).vt_switching = self.keyboard.vt_switching.unwrap_or(true);
            if let Some(msec) = self.repaint_window_msec {
                if !(-10..=1000).contains(&msec) {
                    log::log_line(&format!("Invalid repaint_window value in config: {msec}"));
                } else {
                    (*compositor).repaint_msec = msec;
                }
            }
            log::log_line(&format!(
                "Output repaint window is {} ms maximum.",
                (*compositor).repaint_msec
            ));
            // C main.c:1203, immediately after the repaint window and
            // before the backends load — the colour manager must exist
            // before any output is configured, since that is when
            // profiles and EOTF modes are applied.
            if self.color_management {
                if weston_sys::weston_compositor_load_color_manager(compositor) < 0 {
                    return Err(CompositorError::ColorManager);
                }
                ctx.inner.use_color_manager.set(true);
            }
            // main.c:4653 `wet.compositor->multi_backend = backends &&
            // strchr(backends, ',')`, set before load_backends.  Load
            // bearing in libweston: output_accumulate_damage drops the
            // core buffer reference early ("the backend has seen it")
            // only when a single backend is in play — with two, that
            // optimization releases buffers the second backend still
            // needs (compositor.c:3405).
            (*compositor).multi_backend = self.backends.len() > 1;
        }

        let mut comp = Compositor {
            ctx: ctx.clone(),
            policy_names: self.policy.rules.iter().map(|r| r.name.clone()).collect(),
            log_ctx,
            heads_changed: None,
            frontend_listeners: Vec::new(),
            signal_sources: Vec::new(),
            socket_bound: None,
            exit_code: 0,
        };

        // Signal sources BEFORE backend load, as C installs signals[]
        // right after display creation: wl_event_loop_add_signal blocks
        // the signal via sigprocmask on THIS thread, and threads spawned
        // later inherit that mask.  The VNC backend spawns neatvnc/aml
        // worker threads during load — install after it, and a
        // process-directed SIGTERM can be delivered to a worker (which
        // never blocked it) and kill the process with the default action
        // instead of reaching the signalfd (seen live: CI SIGTERM death
        // in the vnc valgrind leg while the same run passed locally —
        // it is a per-delivery race).
        comp.install_signal_sources()?;

        // Sync-tier heads-changed listener (§3e names this tier: outputs
        // must be configured inside the flush).  It touches only wrapper
        // state + the output policy — a pure data lookup, no app
        // borrow (A3).
        let policy = self.policy.clone();
        // The DRM branch needs two settings the policy does not carry
        // (they are backend-level, not per-output): --current-mode and
        // [core] require-outputs.  Captured here rather than looked up
        // in the handler, so the sync-tier A3 proof stays "wrapper
        // state only, no app borrow".
        let drm_runtime = DrmRuntime {
            current_mode: self
                .backends
                .iter()
                .any(|b| matches!(b, BackendSpec::Drm(o) if o.current_mode)),
            require_outputs: self.require_outputs,
        };
        let listener = Listener::new(
            "heads_changed",
            false,
            Box::new(move |ctx, _data| heads_changed(ctx, &policy, drm_runtime)),
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

        // Frontend mirror listeners (C wet_compositor.output_created_
        // listener at main.c:4245 — L6 — and the wet_head_tracker
        // resized listener — L2).  Installed unconditionally like C;
        // both no-op unless an [[output]] rule carries mirror-of=.
        // Sync tier: pure wrapper/policy work, no app borrow (A3) —
        // enabling the mirror nests inside the source's
        // output_created emission exactly as C's handler does.
        let policy = self.policy.clone();
        let out_created = Listener::new(
            "frontend.output_created",
            false,
            Box::new(move |ctx, data| {
                if let Some(o) = NonNull::new(data.cast::<weston_sys::weston_output>()) {
                    mirror_on_output_created(ctx, &policy, o);
                }
            }),
        );
        let policy = self.policy.clone();
        let out_resized = Listener::new(
            "frontend.output_resized",
            false,
            Box::new(move |ctx, data| {
                if let Some(o) = NonNull::new(data.cast::<weston_sys::weston_output>()) {
                    mirror_on_output_resized(ctx, &policy, o);
                }
            }),
        );
        // SAFETY: compositor live; both signals are fields on it.
        unsafe {
            weston_sys::wsys_wl_signal_add(
                &raw mut (*compositor).output_created_signal,
                out_created.raw_ptr(),
            );
            weston_sys::wsys_wl_signal_add(
                &raw mut (*compositor).output_resized_signal,
                out_resized.raw_ptr(),
            );
        }
        for l in [out_created, out_resized] {
            l.mark_attached();
            // Owned by the Compositor, NOT ctx.own_listener: these
            // nodes sit on signal lists INSIDE the weston_compositor
            // struct, and without a shell attached ctx.teardown() only
            // runs after weston_compositor_destroy has freed it —
            // detaching then is a write into freed memory (caught by
            // the r0 valgrind gate).  Drop detaches them next to
            // heads_changed, before the destroy.
            comp.frontend_listeners.push(l);
        }

        for spec in &self.backends {
            match spec {
                BackendSpec::Headless(opts) => comp.load_headless(self.renderer, opts)?,
                BackendSpec::Vnc(opts) => comp.load_vnc(self.renderer, opts)?,
                BackendSpec::X11(opts) => comp.load_x11(self.renderer, opts)?,
                BackendSpec::Wayland(opts) => comp.load_wayland(self.renderer, opts)?,
                BackendSpec::Pipewire(opts) => comp.load_pipewire(self.renderer, opts)?,
                BackendSpec::Drm(opts) => comp.load_drm(self.renderer, opts)?,
            }
        }

        // main.c:4660 — installs the (no-op) color manager and finalizes
        // backend setup; weston_output_init dereferences the color
        // manager, so this MUST precede the heads flush.
        // SAFETY: compositor live, backends just loaded.
        let ok = unsafe { weston_sys::weston_compositor_backends_loaded(compositor) };
        if ok < 0 {
            return Err(CompositorError::BackendLoad);
        }

        let has_shell = self.shell.is_some();
        #[cfg(feature = "hybrid-r1")]
        if let Some((bg, factory)) = self.shell
            && !crate::shell_init::attach_shell_native(&ctx, bg, factory)
        {
            return Err(CompositorError::ShellInit);
        }
        // C wet_shell_init ends by calling screenshooter_create
        // (shell.c:2232); at R2e the frontend owns that call — same
        // registrations at the same point in startup (plan §4).
        if has_shell {
            let auth = crate::screenshooter::create(&ctx);
            // Its node lives on a signal inside the compositor struct:
            // Compositor-owned, detached in Drop before destroy.
            comp.frontend_listeners.push(auth);
        }

        if self.socket {
            let bound = match &self.socket_name {
                Some(name) => {
                    let cname =
                        CString::new(name.as_str()).map_err(|_| CompositorError::SocketAdd)?;
                    // SAFETY: display live; name is a valid C string for
                    // the call (libwayland copies it).
                    let r = unsafe {
                        weston_sys::wl_display_add_socket(display.as_ptr(), cname.as_ptr())
                    };
                    if r != 0 {
                        return Err(CompositorError::SocketAdd);
                    }
                    name.clone()
                }
                None => {
                    // SAFETY: display is live; the returned name is a
                    // borrowed C string, copied at the fence (§3h).
                    let name = unsafe { weston_sys::wl_display_add_socket_auto(display.as_ptr()) };
                    if name.is_null() {
                        return Err(CompositorError::SocketAdd);
                    }
                    // SAFETY: non-null return is a valid NUL-terminated
                    // string owned by the display.
                    unsafe { std::ffi::CStr::from_ptr(name) }
                        .to_string_lossy()
                        .into_owned()
                }
            };
            crate::log::log_line(&format!("westonite: wayland socket {bound}"));
            comp.socket_bound = Some(bound);
        }

        // The C frontend flushes heads once after backend setup
        // (main.c:4671); our sync-tier listener enables outputs here.
        ctx.with_depth(|| {
            // SAFETY: compositor is live.
            unsafe { weston_sys::weston_compositor_flush_heads_changed(compositor) };
        });

        // C wet_main: `if (wet.init_failed) goto out` immediately after
        // the flush — an output that could not be created, configured
        // or enabled is a startup failure, never a compositor that
        // comes up quietly short of outputs.
        if ctx.inner.init_failed.get() {
            return Err(CompositorError::OutputInit);
        }

        // C wet_main loads xwayland after the socket and shell, before
        // wake ("Load xwayland before other modules", main.c:4717);
        // failure is fatal (goto out).
        if let Some(xserver) = self.xwayland {
            crate::xwayland::load(&ctx, xserver)?;
        }

        // SAFETY: compositor is live.
        unsafe { weston_sys::weston_compositor_wake(compositor) };

        Ok(comp)
    }
}

pub struct Compositor {
    ctx: Ctx,
    /// `[[output]]` section names in file order — the windowed loaders
    /// filter them by prefix to decide which heads to create, as C
    /// walks the config sections (load_x11_backend / load_wayland_backend).
    policy_names: Vec<String>,
    log_ctx: *mut weston_sys::weston_log_context,
    heads_changed: Option<Listener>,
    /// Frontend listeners on compositor-embedded signals (mirror L6 +
    /// L2): detached in Drop before weston_compositor_destroy.
    frontend_listeners: Vec<Listener>,
    signal_sources: Vec<*mut weston_sys::wl_event_source>,
    socket_bound: Option<String>,
    exit_code: i32,
}

impl Compositor {
    pub fn ctx(&self) -> &Ctx {
        &self.ctx
    }

    /// The wayland socket name bound in build(), if any.
    pub fn socket_name(&self) -> Option<&str> {
        self.socket_bound.as_deref()
    }

    /// Watch an autolaunched client: when `watch` and the pid exits,
    /// the compositor terminates (C execute_autolaunch + sigchld).
    pub fn set_autolaunch(&self, pid: i32, watch: bool) {
        self.ctx.inner.autolaunch.set(Some((pid, watch)));
    }

    fn load_headless(
        &mut self,
        renderer: RendererKind,
        opts: &HeadlessOptions,
    ) -> Result<(), CompositorError> {
        let compositor = self.ctx.inner.compositor.get();

        let mut config: weston_sys::weston_headless_backend_config =
            // SAFETY: POD config struct; zeroed then fully initialized.
            unsafe { std::mem::zeroed() };
        config.base.struct_version = weston_sys::WESTON_HEADLESS_BACKEND_CONFIG_VERSION;
        config.base.struct_size = std::mem::size_of::<weston_sys::weston_headless_backend_config>();
        config.renderer = renderer.to_c();
        // C load_headless_backend: -1 unless --refresh-rate was given.
        config.refresh = opts.refresh_mhz.unwrap_or(-1);

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
        // C stores this on the per-backend `wet_backend` that owns the
        // heads-changed listener; the wrapper installs one listener, so
        // the discriminators live on the Ctx (see heads_changed).
        self.ctx
            .inner
            .backends
            .borrow_mut()
            .push((backend, BackendKind::Headless));

        // C load_headless_backend: --no-outputs skips head creation
        // entirely (the compositor runs with zero outputs).
        if opts.no_outputs {
            return Ok(());
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

    /// C load_vnc_backend: assemble the versioned config (the backend
    /// strdup's the strings during load — C frees its copies right
    /// after, so temporaries are the contract) and load.  The VNC
    /// backend creates its own head; there is no create_head step.
    fn load_vnc(
        &mut self,
        renderer: RendererKind,
        opts: &VncOptions,
    ) -> Result<(), CompositorError> {
        let compositor = self.ctx.inner.compositor.get();

        let addr = match &opts.bind_address {
            Some(a) => Some(CString::new(a.as_str()).map_err(|_| CompositorError::BackendLoad)?),
            None => None,
        };
        let cert = match &opts.tls_cert {
            Some(p) => Some(CString::new(p.as_str()).map_err(|_| CompositorError::BackendLoad)?),
            None => None,
        };
        let key = match &opts.tls_key {
            Some(p) => Some(CString::new(p.as_str()).map_err(|_| CompositorError::BackendLoad)?),
            None => None,
        };

        let mut config: weston_sys::weston_vnc_backend_config =
            // SAFETY: POD config struct; zeroed then fully initialized
            // (weston_vnc_backend_config_init + the option/section reads).
            unsafe { std::mem::zeroed() };
        config.base.struct_version = weston_sys::WESTON_VNC_BACKEND_CONFIG_VERSION;
        config.base.struct_size = std::mem::size_of::<weston_sys::weston_vnc_backend_config>();
        config.renderer = renderer.to_c();
        config.bind_address = addr
            .as_ref()
            .map_or(std::ptr::null_mut(), |c| c.as_ptr().cast_mut());
        config.port = opts.port.unwrap_or(5900);
        // VNC_DEFAULT_FREQ (Hz).
        config.refresh_rate = opts.refresh_rate_hz.unwrap_or(60);
        config.server_cert = cert
            .as_ref()
            .map_or(std::ptr::null_mut(), |c| c.as_ptr().cast_mut());
        config.server_key = key
            .as_ref()
            .map_or(std::ptr::null_mut(), |c| c.as_ptr().cast_mut());
        config.disable_tls = opts.disable_tls;

        // SAFETY: compositor live; config + its strings outlive the
        // call, and the backend copies what it keeps (C frees the very
        // same pointers immediately after this call).
        let backend = self.ctx.with_depth(|| unsafe {
            weston_sys::weston_compositor_load_backend(
                compositor,
                weston_sys::weston_compositor_backend::WESTON_BACKEND_VNC,
                &mut config.base,
            )
        });
        if backend.is_null() {
            return Err(CompositorError::BackendLoad);
        }
        self.ctx
            .inner
            .backends
            .borrow_mut()
            .push((backend, BackendKind::Vnc));
        Ok(())
    }

    /// Shared tail of the two windowed nested backends (C's
    /// `weston_windowed_output_get_api` + the create_head loop in
    /// load_x11_backend / load_wayland_backend): create a head per
    /// matching `[[output]]` section name, then default-named heads up
    /// to `count`.  `prefix` is C's name filter ('X' for x11, "WL" for
    /// wayland); `default_name` builds `screenN` / `waylandN`.
    fn create_windowed_heads(
        &mut self,
        backend: *mut weston_sys::weston_backend,
        api_name: &std::ffi::CStr,
        prefix: &str,
        default_name: &dyn Fn(i32) -> String,
        count: i32,
    ) -> Result<(), CompositorError> {
        let compositor = self.ctx.inner.compositor.get();
        // SAFETY: compositor live; the API name/version come from the
        // installed header constants (§3k static-inline pattern).
        let api = unsafe {
            weston_sys::weston_plugin_api_get(
                compositor,
                api_name.as_ptr(),
                std::mem::size_of::<weston_sys::weston_windowed_output_api>(),
            )
        }
        .cast::<weston_sys::weston_windowed_output_api>();
        if api.is_null() {
            log::log_line("Cannot use weston_windowed_output_api.");
            return Err(CompositorError::WindowedOutputApi);
        }

        let mut made = 0;
        // C walks the config sections in file order and takes those
        // whose `name` starts with the backend's prefix, up to count.
        let named: Vec<String> = self
            .policy_names
            .iter()
            .filter(|n| n.starts_with(prefix))
            .cloned()
            .collect();
        for name in named {
            if made >= count {
                break;
            }
            let cname = CString::new(name.as_str()).map_err(|_| CompositorError::HeadCreate)?;
            // SAFETY: api non-null; create_head is set by both windowed
            // backends; the name outlives the call.
            let r = self.ctx.with_depth(|| unsafe {
                ((*api).create_head.expect("create_head"))(backend, cname.as_ptr())
            });
            if r < 0 {
                return Err(CompositorError::HeadCreate);
            }
            made += 1;
        }
        for i in made..count {
            let cname = CString::new(default_name(i)).map_err(|_| CompositorError::HeadCreate)?;
            // SAFETY: as above.
            let r = self.ctx.with_depth(|| unsafe {
                ((*api).create_head.expect("create_head"))(backend, cname.as_ptr())
            });
            if r < 0 {
                return Err(CompositorError::HeadCreate);
            }
        }
        Ok(())
    }

    /// C load_x11_backend (main.c:3924): weston runs as an X client and
    /// each output is an X window.
    fn load_x11(
        &mut self,
        renderer: RendererKind,
        opts: &X11Options,
    ) -> Result<(), CompositorError> {
        let compositor = self.ctx.inner.compositor.get();
        let mut config: weston_sys::weston_x11_backend_config =
            // SAFETY: POD config; zeroed then fully initialized.
            unsafe { std::mem::zeroed() };
        config.base.struct_version = weston_sys::WESTON_X11_BACKEND_CONFIG_VERSION;
        config.base.struct_size = std::mem::size_of::<weston_sys::weston_x11_backend_config>();
        config.renderer = renderer.to_c();
        config.fullscreen = opts.fullscreen;
        config.no_input = opts.no_input;

        // SAFETY: compositor live; the backend copies what it keeps.
        let backend = self.ctx.with_depth(|| unsafe {
            weston_sys::weston_compositor_load_backend(
                compositor,
                weston_sys::weston_compositor_backend::WESTON_BACKEND_X11,
                &mut config.base,
            )
        });
        if backend.is_null() {
            return Err(CompositorError::BackendLoad);
        }
        self.ctx
            .inner
            .backends
            .borrow_mut()
            .push((backend, BackendKind::X11));

        let count = opts.output_count.max(1);
        self.create_windowed_heads(
            backend,
            c"weston_windowed_output_api_x11_v2",
            "X",
            &|i| format!("screen{i}"),
            count,
        )
    }

    /// C load_wayland_backend (main.c:4044): weston nested inside
    /// another Wayland compositor, one output per parent surface.
    fn load_wayland(
        &mut self,
        renderer: RendererKind,
        opts: &WaylandOptions,
    ) -> Result<(), CompositorError> {
        let compositor = self.ctx.inner.compositor.get();
        let display = match &opts.display_name {
            Some(d) => Some(CString::new(d.as_str()).map_err(|_| CompositorError::BackendLoad)?),
            None => None,
        };
        let theme = match &opts.cursor_theme {
            Some(t) => Some(CString::new(t.as_str()).map_err(|_| CompositorError::BackendLoad)?),
            None => None,
        };

        let mut config: weston_sys::weston_wayland_backend_config =
            // SAFETY: POD config; zeroed then fully initialized.
            unsafe { std::mem::zeroed() };
        config.base.struct_version = weston_sys::WESTON_WAYLAND_BACKEND_CONFIG_VERSION;
        config.base.struct_size = std::mem::size_of::<weston_sys::weston_wayland_backend_config>();
        config.renderer = renderer.to_c();
        config.sprawl = opts.sprawl;
        config.fullscreen = opts.fullscreen;
        config.display_name = display
            .as_ref()
            .map_or(std::ptr::null_mut(), |c| c.as_ptr().cast_mut());
        config.cursor_theme = theme
            .as_ref()
            .map_or(std::ptr::null_mut(), |c| c.as_ptr().cast_mut());
        config.cursor_size = if opts.cursor_size > 0 {
            opts.cursor_size
        } else {
            32
        };

        // SAFETY: compositor live; C frees its copies of these strings
        // right after the same call, so temporaries are the contract.
        let backend = self.ctx.with_depth(|| unsafe {
            weston_sys::weston_compositor_load_backend(
                compositor,
                weston_sys::weston_compositor_backend::WESTON_BACKEND_WAYLAND,
                &mut config.base,
            )
        });
        if backend.is_null() {
            return Err(CompositorError::BackendLoad);
        }
        self.ctx
            .inner
            .backends
            .borrow_mut()
            .push((backend, BackendKind::Wayland));

        // C: with --sprawl (or a fullscreen-shell parent) the windowed
        // API is absent and everything is hardcoded — create nothing
        // and let the backend bring its own output up.
        if opts.sprawl {
            return Ok(());
        }
        let count = opts.output_count.max(1);
        self.create_windowed_heads(
            backend,
            c"weston_windowed_output_api_wayland_v2",
            "WL",
            &|i| format!("wayland{i}"),
            count,
        )
    }

    /// C load_pipewire_backend (main.c:3611): outputs are published as
    /// PipeWire streams.  The backend creates its own heads from
    /// `num_outputs`; there is no create_head loop.
    fn load_pipewire(
        &mut self,
        renderer: RendererKind,
        opts: &PipewireOptions,
    ) -> Result<(), CompositorError> {
        let compositor = self.ctx.inner.compositor.get();
        let fmt = match &opts.gbm_format {
            Some(f) => Some(CString::new(f.as_str()).map_err(|_| CompositorError::BackendLoad)?),
            None => None,
        };

        let mut config: weston_sys::weston_pipewire_backend_config =
            // SAFETY: POD config; zeroed then fully initialized (C
            // weston_pipewire_backend_config_init sets the same two
            // base fields).
            unsafe { std::mem::zeroed() };
        config.base.struct_version = weston_sys::WESTON_PIPEWIRE_BACKEND_CONFIG_VERSION;
        config.base.struct_size = std::mem::size_of::<weston_sys::weston_pipewire_backend_config>();
        config.renderer = renderer.to_c();
        config.gbm_format = fmt
            .as_ref()
            .map_or(std::ptr::null_mut(), |c| c.as_ptr().cast_mut());
        config.num_outputs = opts.num_outputs.max(1);

        // SAFETY: compositor live; config + strings outlive the call.
        let backend = self.ctx.with_depth(|| unsafe {
            weston_sys::weston_compositor_load_backend(
                compositor,
                weston_sys::weston_compositor_backend::WESTON_BACKEND_PIPEWIRE,
                &mut config.base,
            )
        });
        if backend.is_null() {
            return Err(CompositorError::BackendLoad);
        }
        self.ctx
            .inner
            .backends
            .borrow_mut()
            .push((backend, BackendKind::Pipewire));
        Ok(())
    }

    /// C load_drm_backend (main.c:3375).  The DRM backend creates no
    /// head on load: it discovers connectors itself and the
    /// heads-changed listener does the rest, via the layoutput
    /// machinery rather than the simple path (see [`crate::layoutput`]).
    ///
    /// `config.configure_device` is deliberately left null — see the
    /// `[libinput]` refusal in the frontend.  C points it at
    /// `configure_input_device`, which needs libinput's own device API;
    /// binding that is its own slice, so rather than silently ignore
    /// `[libinput]` we refuse it at startup while DRM is loaded.
    fn load_drm(
        &mut self,
        renderer: RendererKind,
        opts: &DrmOptions,
    ) -> Result<(), CompositorError> {
        let compositor = self.ctx.inner.compositor.get();

        let cstr = |s: &Option<String>| -> Result<Option<CString>, CompositorError> {
            match s {
                Some(v) => Ok(Some(
                    CString::new(v.as_str()).map_err(|_| CompositorError::BackendLoad)?,
                )),
                None => Ok(None),
            }
        };
        let seat = cstr(&opts.seat_id)?;
        let device = cstr(&opts.specific_device)?;
        let additional = cstr(&opts.additional_devices)?;
        let gbm = cstr(&opts.gbm_format)?;

        let mut config: weston_sys::weston_drm_backend_config =
            // SAFETY: POD config struct; zeroed then fully initialized.
            unsafe { std::mem::zeroed() };
        config.base.struct_version = weston_sys::WESTON_DRM_BACKEND_CONFIG_VERSION;
        config.base.struct_size = std::mem::size_of::<weston_sys::weston_drm_backend_config>();
        config.renderer = renderer.to_c();
        let ptr = |c: &Option<CString>| {
            c.as_ref()
                .map_or(std::ptr::null_mut(), |v| v.as_ptr().cast_mut())
        };
        config.seat_id = ptr(&seat);
        config.specific_device = ptr(&device);
        config.additional_devices = ptr(&additional);
        config.gbm_format = ptr(&gbm);
        config.pageflip_timeout = opts.pageflip_timeout;
        config.use_pixman_shadow = opts.use_pixman_shadow;

        // C: `if (without_input) c->require_input = !without_input;` --
        // only ever clears the flag, never sets it (main.c:3419).
        if opts.continue_without_input {
            // SAFETY: compositor live; require_input is a bound POD
            // field the backend reads during load.
            unsafe { (*compositor).require_input = false };
        }

        // C warn_possible_tty(): a DRM compositor started from inside
        // another compositor's terminal is a common mistake and the
        // failure mode is opaque, so C says so first.
        if std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("DISPLAY").is_some() {
            log::log_line(
                "Warning: loading the DRM backend from inside a graphical session; \
                 this usually fails to take DRM master.",
            );
        }

        // SAFETY: compositor live; config + strings outlive the call.
        let backend = self.ctx.with_depth(|| unsafe {
            weston_sys::weston_compositor_load_backend(
                compositor,
                weston_sys::weston_compositor_backend::WESTON_BACKEND_DRM,
                &mut config.base,
            )
        });
        if backend.is_null() {
            return Err(CompositorError::BackendLoad);
        }
        self.ctx
            .inner
            .backends
            .borrow_mut()
            .push((backend, BackendKind::Drm));
        Ok(())
    }

    /// Install the SIGTERM/SIGINT/SIGCHLD event-loop sources (main.c
    /// signals[]).  Called from build() BEFORE backend load — the
    /// sigprocmask that backs the signalfd must be in place before any
    /// backend spawns threads (they inherit the mask; see the call
    /// site) — which also puts it before any client spawn (SIGCHLD
    /// reaping/watch never misses an early exit).
    fn install_signal_sources(&mut self) -> Result<(), CompositorError> {
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
        // SIGCHLD: reap clients; terminate on watched-autolaunch exit
        // (C main.c sigchld_handler + autolaunch watch).
        // SAFETY: as s1 — same display, same loop.
        let s3 = unsafe {
            weston_sys::wl_event_loop_add_signal(
                ev_loop,
                libc::SIGCHLD,
                Some(on_sigchld),
                display.cast(),
            )
        };
        if s1.is_null() || s2.is_null() || s3.is_null() {
            // Remove whichever source did install, or it would stay
            // registered (and leak) past this failed attempt.
            for s in [s1, s2, s3] {
                if !s.is_null() {
                    // SAFETY: source just created on this loop, not yet
                    // tracked anywhere else.
                    unsafe { weston_sys::wl_event_source_remove(s) };
                }
            }
            return Err(CompositorError::SignalSource);
        }
        // Removed in Drop, as main.c removes its signals[] sources.
        self.signal_sources.push(s1);
        self.signal_sources.push(s2);
        self.signal_sources.push(s3);

        // C main.c:4570: block SIGUSR1 up front, unconditionally —
        // "Xwayland uses SIGUSR1 for communicating with weston" — so
        // that (a) a stray SIGUSR1 can't kill the process with its
        // default action, and (b) backend worker threads spawned later
        // inherit the block.  Same placement rationale as the signalfd
        // masks above.
        // SAFETY: plain sigset syscalls on a stack-local set.
        unsafe {
            let mut mask: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut mask);
            libc::sigaddset(&mut mask, libc::SIGUSR1);
            libc::pthread_sigmask(libc::SIG_BLOCK, &mask, std::ptr::null_mut());
        }
        Ok(())
    }

    /// Run until terminated (SIGTERM/SIGINT).  Returns the process exit
    /// code (0 on clean signal-driven shutdown — the Phase-1 contract).
    pub fn run(&mut self) -> i32 {
        let display = self.ctx.inner.display.get();
        // Deliberately NOT wrapped in `with_depth`, unlike every other
        // outbound call: this one *is* the event loop, so holding the
        // counter at 1 for its whole lifetime would mean the counter
        // never returns to zero while the compositor runs, and A4's
        // drain-at-the-edge could only fire after the loop exits.  That
        // was the R0 shape, and it made every deferred-tier event
        // (focus-after-close, busy-cursor end, GrabEnded, the Gone
        // policies) land in one batch at shutdown, while retired boxes
        // piled up in the pending-drop list for the whole session.
        // With the base depth at zero, each trampoline's own wrap goes
        // 0→1→0 and drains at its edge — which is exactly where the C
        // code ran the same handler.
        //
        // SAFETY: the run loop dispatches into our trampolines; every
        // one re-enters through Ctx::current + with_depth.  Full audit
        // of the entry points that deliberately do NOT wrap:
        //   - the two event-loop signal callbacks below — they wrap
        //     their own bodies instead, so one handler is one drain;
        //   - the label / no-op vtable entries (curtain_get_label,
        //     curtain_committed, and the empty grab entries — the
        //     noop_* set plus touch_down/touch_frame) and the
        //     weston_log sink, which must stay unwrapped: draining from
        //     inside the log sink would run app handlers on a log call;
        //   - desktop::tramp_get_position, which answers from
        //     wrapper-held view state into out-params and makes no
        //     outbound call at all, so it has nothing to drain;
        //   - desktop::client_surfaces' nested `collect`, which is not
        //     an entry point on C's schedule at all: we hand it to
        //     weston_desktop_client_for_each_surface from inside an
        //     already-wrapped trampoline, and it only appends to a
        //     caller-owned Vec.
        // (Re-derive with a body-scoped scan for extern "C" fns whose
        // bodies contain no with_depth/with_ctx/guard_ctx, not a
        // fixed-size window — the window version missed two of these.)
        unsafe { weston_sys::wl_display_run(display) };
        self.exit_code
    }
}

impl Drop for Compositor {
    fn drop(&mut self) {
        // Teardown order mirrors main.c's `out:` path; shutting_down
        // first so destroy-storm policy events are discarded (§3e).
        self.ctx.inner.shutting_down.set(true);
        // C wet_xwayland_destroy runs before weston_compositor_destroy
        // (the module must still be live for xserver_exited).
        crate::xwayland::teardown(&self.ctx);
        // Screenshooter: detach the per-client listener before the
        // display (and its clients) dies; the authority listener is in
        // frontend_listeners below (C screenshooter_destroy removes
        // both from the compositor's destroy emission).
        crate::screenshooter::teardown(&self.ctx);
        if let Some(l) = self.heads_changed.take() {
            l.detach();
        }
        for l in self.frontend_listeners.drain(..) {
            // SAFETY-relevant ordering: the signal lists these sit on
            // live inside the compositor struct, freed just below.
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

extern "C" fn on_sigchld(_signal: c_int, data: *mut c_void) -> c_int {
    crate::panic_barrier::guard("on_sigchld", || {
        // Event-loop callbacks run at the loop's base depth, which is
        // zero (see `run`), so the whole body takes the +1 itself: the
        // outbound calls below (xserver_exited, weston_log,
        // wl_display_terminate) would otherwise each return the counter
        // to zero and drain mid-reap, running shell handlers between
        // two waitpid iterations.  One wrap ⇒ one drain, at the edge.
        //
        // Resolved once, up front — but the reap must NOT depend on it.
        // A missing Ctx here would be a teardown-ordering bug (§3j; not
        // reachable today, since Drop removes the signal sources before
        // Ctx::teardown), and the one thing a SIGCHLD handler may never
        // skip is waitpid: bailing out would leave real zombies behind
        // for a wrapper-state problem.  Reap either way; only the
        // frontend bookkeeping and the depth wrap need the Ctx.
        let ctx = Ctx::current();
        debug_assert!(ctx.is_some(), "on_sigchld with no live Ctx");
        // Reap every exited child (C sigchld_handler's waitpid loop).
        let reap = || {
            loop {
                let mut status: c_int = 0;
                // SAFETY: plain waitpid; WNOHANG never blocks the loop.
                let pid = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG) };
                if pid <= 0 {
                    break;
                }
                let Some(ctx) = ctx.as_ref() else { continue };
                if let Some((watched, watch)) = ctx.inner.autolaunch.get()
                    && pid == watched
                {
                    ctx.inner.autolaunch.set(None);
                    if watch {
                        crate::log::log_line("westonite: autolaunched client exited, terminating");
                        // SAFETY: data is the live wl_display registered
                        // with this source.
                        unsafe { weston_sys::wl_display_terminate(data.cast()) };
                    }
                    continue;
                }
                // Frontend-tracked children (C sigchld_handler's
                // wet_process walk): the Xwayland server (logged + relayed
                // to the module, which respawns on the next X connection)
                // and the screenshooter client (logged only).
                if crate::xwayland::handle_child_exit(ctx, pid, status) {
                    continue;
                }
                crate::screenshooter::handle_child_exit(ctx, pid, status);
            }
        };
        match ctx.as_ref() {
            Some(c) => c.with_depth(reap),
            None => reap(),
        }
    });
    1
}

extern "C" fn on_term_signal(_signal: c_int, data: *mut c_void) -> c_int {
    // Async-safe enough: wl_event_loop signal sources deliver via
    // signalfd on the main loop, not in async signal context.
    //
    // Wrapped like on_sigchld now that the loop's base depth is zero
    // (see `run`).  wl_display_terminate only clears the loop's run
    // flag — it emits nothing and cannot re-enter us — so this is A4
    // hygiene rather than a live hazard.  A Ctx-less late signal still
    // terminates, just unwrapped: the display outlives the Ctx in
    // Compositor::drop, and dropping a SIGTERM on the floor would be a
    // worse failure than skipping a drain with nothing to drain.
    let Some(ctx) = Ctx::current() else {
        // SAFETY: as below; without a Ctx there is nothing to drain.
        unsafe { weston_sys::wl_display_terminate(data.cast()) };
        return 1;
    };
    ctx.with_depth(|| {
        // SAFETY: data is the live wl_display registered above.
        unsafe { weston_sys::wl_display_terminate(data.cast()) };
    });
    1
}

/// The registry side of head/output tracking (§3b): registration and
/// destroy-listener attachment in one place, invoked from the sync-tier
/// heads-changed handler.  C simple_heads_changed, all three branches.
fn heads_changed(ctx: &Ctx, policy: &OutputPolicy, drm: DrmRuntime) {
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

    // C iterates only the heads of the backend whose wet_backend owns
    // this listener (wet_backend_iterate_heads), and each wet_backend
    // carries its own simple_output_configure.  One listener here, so
    // both jobs are done by lookup: a head whose backend we did not
    // load is skipped, and the kind picks the configure flavor.
    let backends = ctx.inner.backends.borrow().clone();
    let mut saw_drm = false;

    for head in heads {
        // SAFETY: head snapshot entries stay valid inside the flush; the
        // is_* getters are pure queries and `backend` is a bound POD
        // field set by the backend that created the head.
        let (head_backend, connected, enabled, changed, non_desktop) = unsafe {
            (
                (*head.as_ptr()).backend,
                weston_sys::weston_head_is_connected(head.as_ptr()),
                weston_sys::weston_head_is_enabled(head.as_ptr()),
                weston_sys::weston_head_is_device_changed(head.as_ptr()),
                weston_sys::weston_head_is_non_desktop(head.as_ptr()),
            )
        };
        let Some(kind) = backends
            .iter()
            .find(|(b, _)| *b == head_backend)
            .map(|(_, k)| *k)
        else {
            continue;
        };
        // C gives DRM its own heads-changed handler
        // (`drm_heads_changed`, main.c:3006) rather than a branch of
        // the simple one, because the enable step is a *group*
        // operation: heads are staged on a layoutput and enabled
        // together, after this loop.  Same split here.
        if kind == BackendKind::Drm {
            let name = head_name(head);
            let forced = policy.drm_force_on(&name);
            if (connected || forced) && !enabled {
                drm_prepare_enable(ctx, policy, &name, head, non_desktop, drm.current_mode);
            } else if !(connected || forced) && enabled {
                drm_head_disable(ctx, head);
            } else if enabled && changed {
                log::log_line(&format!(
                    "Detected a monitor change on head '{name}', \
                     not bothering to do anything about it."
                ));
            }
            // SAFETY: head live in the snapshot; C resets per head.
            unsafe { weston_sys::weston_head_reset_device_changed(head.as_ptr()) };
            saw_drm = true;
            continue;
        }
        if connected && !enabled && !non_desktop {
            let name = head_name(head);
            // C simple_head_enable's remote-mirror deferral: a remote
            // head whose section carries mirror-of= waits for its
            // source output — the frontend's output-created listener
            // enables it (silent skip, like C's early return; the head
            // stays connected-unenabled meanwhile, and the per-head
            // device-changed reset below still runs, as in C).
            if kind == BackendKind::Vnc && policy.has_mirror_of(&name) {
                // deferred to the output-created path
            } else {
                let setup = head_setup_for(policy, kind, &name);
                match setup {
                    Some(setup) => enable_head(ctx, head, setup, None, &policy.decide_color(&name)),
                    None => {
                        // [[output]] off: leave the head unenabled (our
                        // re-spec extension; C windowed backends have no
                        // per-output off switch).  The head stays
                        // connected-and-unenabled forever, so every later
                        // flush would re-log: register_head returning true
                        // (first sight) is the once-per-head gate.
                        if register_head(ctx, head, &name) {
                            log::log_line(&format!("westonite: output {name} disabled by config"));
                        }
                    }
                }
            }
        } else if !connected && enabled {
            disable_head(head);
        } else if enabled && changed {
            log::log_line(&format!(
                "Detected a monitor change on head '{}', not bothering to do anything about it.",
                head_name(head)
            ));
        }
        // SAFETY: head live in the snapshot (nothing above frees heads;
        // disable destroys the *output*); C resets the flag per head.
        unsafe { weston_sys::weston_head_reset_device_changed(head.as_ptr()) };
    }

    // C drm_heads_changed's tail: once every head of this flush is
    // staged, bring the groups up.  A failure here is fatal to startup
    // (C sets wet->init_failed), which is how `mode=off` on the only
    // head, or a layoutput that would not enable, stops the compositor
    // instead of leaving it running blind.
    if saw_drm && !crate::layoutput::process_all(ctx, policy, drm.current_mode, drm.require_outputs)
    {
        ctx.inner.init_failed.set(true);
    }
}

/// C `drm_head_prepare_enable` (main.c:2764): decide which layoutput
/// this head joins, or that it joins none.
fn drm_prepare_enable(
    ctx: &Ctx,
    policy: &OutputPolicy,
    name: &str,
    head: NonNull<weston_sys::weston_head>,
    non_desktop: bool,
    current_mode: bool,
) {
    use crate::output_policy::DrmHeadPlan;
    match policy.decide_drm(name, non_desktop, current_mode) {
        DrmHeadPlan::Enable(plan) => {
            crate::layoutput::add_head(ctx, &plan.output_name, name, head);
        }
        // mode=off, or a non-desktop head with no section: C returns
        // without staging and lets the backend turn the head off.
        DrmHeadPlan::Off | DrmHeadPlan::NonDesktop => {}
        DrmHeadPlan::BadCloneOf(message) => {
            // C logs and treats the head as having no section at all.
            log::log_line(&message);
            crate::layoutput::add_head(ctx, name, name, head);
        }
    }
}

/// C `drm_head_disable` (main.c:2985): detach, and destroy the output
/// once its last head has gone.
fn drm_head_disable(ctx: &Ctx, head: NonNull<weston_sys::weston_head>) {
    // SAFETY: head live in the snapshot; pure query.
    let output = unsafe { weston_sys::weston_head_get_output(head.as_ptr()) };
    // SAFETY: head live and attached.
    unsafe { weston_sys::weston_head_detach(head.as_ptr()) };
    let Some(output) = NonNull::new(output) else {
        return;
    };
    crate::layoutput::head_detached(ctx, output);
}

/// The kind of the loaded backend that owns `backend`, if we loaded it
/// (C keys this off `weston_get_backend_type` / walks its
/// `backend_list` in `wet_get_backend_from_head`; one listener here, so
/// our own load-order table is the map).
fn backend_kind(ctx: &Ctx, backend: *mut weston_sys::weston_backend) -> Option<BackendKind> {
    ctx.inner
        .backends
        .borrow()
        .iter()
        .find(|(b, _)| *b == backend)
        .map(|(_, k)| *k)
}

/// C wet_head_find_by_name: scan the compositor's heads for `name`.
fn find_head_by_name(ctx: &Ctx, name: &str) -> Option<NonNull<weston_sys::weston_head>> {
    let compositor = ctx.inner.compositor.get();
    if compositor.is_null() {
        return None;
    }
    let mut iter: *mut weston_sys::weston_head = std::ptr::null_mut();
    loop {
        // SAFETY: compositor live; supported enumeration API.
        iter = unsafe { weston_sys::weston_compositor_iterate_heads(compositor, iter) };
        let head = NonNull::new(iter)?;
        if head_name(head) == name {
            return Some(head);
        }
    }
}

/// C wet_output_handle_create (L6): a native output was created — if
/// an [[output]] rule mirrors it, enable that rule's (deferred) remote
/// head now, with this output as the source.
fn mirror_on_output_created(
    ctx: &Ctx,
    policy: &OutputPolicy,
    output: NonNull<weston_sys::weston_output>,
) {
    // C ignores outputs created by remote backends (RDP/VNC/PIPEWIRE):
    // a remote output can never be a mirror source.
    // SAFETY: output live in its creation signal; backend is a bound
    // POD field.
    let out_backend = unsafe { (*output.as_ptr()).backend };
    if backend_kind(ctx, out_backend) == Some(BackendKind::Vnc) {
        return;
    }
    let source_name = output_name(output);
    let Some(rule) = policy.mirror_rule_for_source(&source_name) else {
        return;
    };
    let Some(head) = find_head_by_name(ctx, &rule.name) else {
        return;
    };
    // Robustness divergence from C (documented in the PROVENANCE log):
    // C re-enables without checking, which double-creates an output if
    // a non-deferred head ends up here (e.g. mirror-of on a native
    // section); an already-enabled head is left alone instead.
    // SAFETY: head live; pure query.
    if unsafe { weston_sys::weston_head_is_enabled(head.as_ptr()) } {
        return;
    }
    // C wet_get_backend_from_head + `assert(wb)`: the configurator is
    // the mirrored head's OWN backend's, never a fixed one — so the
    // kind decides, and a head from a backend we did not load is
    // skipped where C would assert.
    // SAFETY: head live; `backend` is a bound POD field.
    let head_backend = unsafe { (*head.as_ptr()).backend };
    let Some(kind) = backend_kind(ctx, head_backend) else {
        return;
    };
    let setup = head_setup_for(policy, kind, &rule.name);
    let Some(setup) = setup else {
        return; // off = true wins over mirror-of
    };
    enable_head(
        ctx,
        head,
        setup,
        Some(output),
        &policy.decide_color(&rule.name),
    );
    // C wet_output_handle_create resets the flag right after the
    // enable, and it is load-bearing: this head was created with
    // device_changed set (weston_head_set_monitor_strings), and the
    // heads-changed flush we are nested inside still has it to visit —
    // now enabled — so without the reset it takes the `enabled &&
    // changed` branch and logs a spurious monitor-change line.
    // SAFETY: head live; plain flag write on it.
    unsafe { weston_sys::weston_head_reset_device_changed(head.as_ptr()) };
}

/// C simple_heads_output_sharing_resize (L2): the mirror SOURCE
/// resized — move the remote over it and recompute its modeline.
fn mirror_on_output_resized(
    ctx: &Ctx,
    policy: &OutputPolicy,
    resized: NonNull<weston_sys::weston_output>,
) {
    let source_name = output_name(resized);
    let Some(rule) = policy.mirror_rule_for_source(&source_name) else {
        return;
    };
    let Some(head) = find_head_by_name(ctx, &rule.name) else {
        return;
    };
    // SAFETY: head live; getter is pure.
    let remote = unsafe { weston_sys::weston_head_get_output(head.as_ptr()) };
    let Some(remote) = NonNull::new(remote) else {
        return; // mirror not (yet) enabled
    };
    // SAFETY: both outputs live inside the resized emission; the
    // mirror_of check pins that `remote` really mirrors `resized`.
    unsafe {
        if (*remote.as_ptr()).mirror_of != resized.as_ptr() {
            return;
        }
        weston_sys::weston_output_set_position(remote.as_ptr(), (*resized.as_ptr()).pos);
    }
    apply_mirror_modeline(ctx, &rule.name, resized, remote);
}

pub(crate) fn output_name(output: NonNull<weston_sys::weston_output>) -> String {
    // SAFETY: output live; name copied at the fence (§3h).
    unsafe {
        let p = (*output.as_ptr()).name;
        if p.is_null() {
            String::new()
        } else {
            std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned()
        }
    }
}

/// C simple_head_disable: destroy the head's output.  Registry
/// invalidation and the policy event ride the output-destroy listener
/// installed at registration.
fn disable_head(head: NonNull<weston_sys::weston_head>) {
    // SAFETY: head live (snapshot inside the flush); getter is pure.
    let output = unsafe { weston_sys::weston_head_get_output(head.as_ptr()) };
    if output.is_null() {
        return;
    }
    // SAFETY: enabled head implies a live output owned by the
    // compositor; destroy emits our destroy listener (§3b invalidation).
    unsafe { weston_sys::weston_output_destroy(output) };
}

/// C weston_output_lazy_align (frontend-local): place the new output to
/// the right of the most recently enabled one.
///
/// Only *enabled* outputs are on `output_list`
/// (weston_compositor_create_output leaves the new one on
/// `pending_output_list` until weston_output_enable moves it), so the
/// output being placed is never its own peer — same as in C.  With a
/// single head the list is empty here and the peer branch does not run,
/// which is why no headless test can reach it; the multi-output
/// coverage arrives with the second backend at R2c.
pub(crate) fn lazy_align(
    compositor: *mut weston_sys::weston_compositor,
    output: NonNull<weston_sys::weston_output>,
) {
    // SAFETY: compositor live; output_list is its embedded list head.
    // The tail entry (prev) is a live weston_output's `link` when the
    // list is non-empty (wl_list_empty <=> prev points at the head).
    unsafe {
        let list: *mut weston_sys::wl_list = &raw mut (*compositor).output_list;
        let prev = (*list).prev;
        let mut next_x = 0.0_f64;
        if !prev.is_null() && prev != list {
            let peer = crate::container_of!(prev, weston_sys::weston_output, link);
            next_x = (*peer).pos.c.x + f64::from((*peer).width);
        }
        (*output.as_ptr()).pos.c = weston_sys::weston_coord { x: next_x, y: 0.0 };
    }
}

pub(crate) fn head_name(head: NonNull<weston_sys::weston_head>) -> String {
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

/// C-owned strdup of an optional string (for APIs where libweston
/// stores the pointer and frees it with free()); null when None.
///
/// SAFETY contract: the CString temporary lives across the strdup call
/// only; the returned pointer is malloc'd by C and owned by the callee.
fn c_strdup_opt(s: Option<&str>) -> *const libc::c_char {
    match s.and_then(|v| CString::new(v).ok()) {
        // SAFETY: valid NUL-terminated input; strdup allocates with the
        // C allocator, matching the free() in libweston's teardown.
        Some(c) => unsafe { libc::strdup(c.as_ptr()) },
        None => std::ptr::null(),
    }
}

/// The per-backend configure decision (C `wb->simple_output_configure`).
enum HeadSetup {
    /// Every windowed backend shares C's
    /// `wet_configure_windowed_output_from_config`; only the plugin API
    /// it drives differs per backend.
    Windowed(OutputSetup, WindowedApi),
    Vnc(crate::output_policy::VncOutputSetup),
    Pipewire(crate::output_policy::PipewireOutputSetup),
}

/// Which `weston_windowed_output_api` instance a windowed head uses
/// (the names are per-backend; C picks with WESTON_WINDOWED_OUTPUT_*).
#[derive(Debug, Clone, Copy, PartialEq)]
enum WindowedApi {
    Headless,
    X11,
    Wayland,
}

impl WindowedApi {
    fn name(self) -> &'static std::ffi::CStr {
        match self {
            WindowedApi::Headless => c"weston_windowed_output_api_headless_v2",
            WindowedApi::X11 => c"weston_windowed_output_api_x11_v2",
            WindowedApi::Wayland => c"weston_windowed_output_api_wayland_v2",
        }
    }
}

/// `mirror_of`: the source output when this head mirrors one (C
/// simple_head_enable's `head_to_mirror` +
/// wet_output_overlap_pre/post_enable pair) — placement and the final
/// modeline then come from the source instead of lazy_align.
/// The per-backend configurator choice (C's `wb->simple_output_configure`
/// slot): which flavor of output setup this backend's heads take, and
/// with which defaults.  `None` means the policy disabled the head.
fn head_setup_for(policy: &OutputPolicy, kind: BackendKind, name: &str) -> Option<HeadSetup> {
    match kind {
        // headless_backend_output_configure, defaults 1024x640.
        BackendKind::Headless => policy
            .decide(name)
            .map(|s| HeadSetup::Windowed(s, WindowedApi::Headless)),
        // x11_backend_output_configure, defaults 1024x600 (main.c:3910).
        BackendKind::X11 => policy
            .decide_sized(name, (1024, 600))
            .map(|s| HeadSetup::Windowed(s, WindowedApi::X11)),
        // wayland_backend_output_configure, defaults 1024x640 (4030).
        BackendKind::Wayland => policy
            .decide_sized(name, (1024, 640))
            .map(|s| HeadSetup::Windowed(s, WindowedApi::Wayland)),
        BackendKind::Vnc => policy.decide_vnc(name).map(HeadSetup::Vnc),
        BackendKind::Pipewire => policy.decide_pipewire(name).map(HeadSetup::Pipewire),
        // DRM has no `simple_output_configure` at all: C gives it its
        // own heads-changed handler and the layoutput machinery, and
        // `heads_changed` below routes it there before ever reaching
        // this table.  Returning None here would silently leave every
        // DRM head unenabled, so assert the routing instead of relying
        // on it (debug_assert: the release path still degrades to the
        // visible "disabled by config" log rather than to UB).
        BackendKind::Drm => {
            debug_assert!(false, "DRM heads go through the layoutput path");
            None
        }
    }
}

/// The colour slice of an output's configuration, applied to a
/// not-yet-enabled output.  Returns false on a configuration error, in
/// which case the caller must abandon the output as C does.
///
/// C splits this across four helpers called from each backend's
/// configure — `wet_output_set_color_profile` (main.c:1365),
/// `_set_eotf_mode` (1403), `_set_colorimetry_mode` (1464),
/// `_set_color_characteristics` (1644) — plus
/// `allow_content_protection` (1679).  One function here, because the
/// only thing that differs between backends is *which* of them get
/// called, and that is the caller's business.
pub(crate) fn apply_color(
    ctx: &Ctx,
    output: NonNull<weston_sys::weston_output>,
    color: &crate::output_policy::ColorSetup,
    full: bool,
) -> bool {
    let compositor = ctx.inner.compositor.get();
    let have_cm = ctx.inner.use_color_manager.get();
    let name = output_name(output);

    // C allow_content_protection: unconditional, every backend, and it
    // cannot fail.
    // SAFETY: output live and not yet enabled.
    unsafe { weston_sys::weston_output_allow_protection(output.as_ptr(), color.allow_hdcp) };

    // C wet_output_set_color_profile: a no-op without the colour
    // manager, *including* the icc_profile key — C returns 0 before
    // even reading it, so an icc-profile without color-management=true
    // is silently ignored there.  We refuse that combination in the
    // frontend instead, so reaching here with one means the manager is
    // loaded.
    if have_cm && let Some(icc) = &color.icc_profile {
        let Ok(path) = CString::new(icc.as_str()) else {
            return false;
        };
        // SAFETY: compositor live; the loader copies what it needs.
        let cprof =
            unsafe { weston_sys::weston_compositor_load_icc_file(compositor, path.as_ptr()) };
        if cprof.is_null() {
            return false;
        }
        // SAFETY: cprof owned by us until unref; output live.
        let ok = unsafe { weston_sys::weston_output_set_color_profile(output.as_ptr(), cprof) };
        // SAFETY: releasing our reference, exactly as C does whether or
        // not the set succeeded.
        unsafe { weston_sys::weston_color_profile_unref(cprof) };
        if !ok {
            log::log_line(&format!(
                "Error: failed to set color profile '{icc}' for output {name}"
            ));
            return false;
        }
    }

    // The rest is DRM/headless-only in C (the windowed and remote
    // configures call set_color_profile and nothing else).
    if !full {
        return true;
    }

    // SAFETY: output live; both are pure queries, and the setters take
    // a mode this build's headers define.
    unsafe {
        let eotf = color.eotf_mode.unwrap_or_default();
        let supported = weston_sys::weston_output_get_supported_eotf_modes(output.as_ptr());
        if supported & eotf.to_c() == 0 {
            log::log_line(&format!(
                "Error: output '{name}' does not support EOTF mode {}.",
                crate::output_policy::EotfMode::NAMES[eotf as usize]
            ));
            return false;
        }
        weston_sys::weston_output_set_eotf_mode(output.as_ptr(), eotf.to_c());

        let cmode = color.colorimetry_mode.unwrap_or_default();
        let supported = weston_sys::weston_output_get_supported_colorimetry_modes(output.as_ptr());
        if supported & cmode.to_c() == 0 {
            log::log_line(&format!(
                "Error: output '{name}' does not support colorimetry mode {}.",
                crate::output_policy::ColorimetryMode::NAMES[cmode as usize]
            ));
            return false;
        }
        weston_sys::weston_output_set_colorimetry_mode(output.as_ptr(), cmode.to_c());

        if let Some(cc) = &color.characteristics {
            let mut c: weston_sys::weston_color_characteristics = std::mem::zeroed();
            c.group_mask = cc.group_mask();
            if let Some(p) = cc.primaries {
                for (i, (x, y)) in p.iter().enumerate() {
                    c.primary[i].x = *x;
                    c.primary[i].y = *y;
                }
            }
            if let Some((x, y)) = cc.white {
                c.white.x = x;
                c.white.y = y;
            }
            c.max_luminance = cc.max_luminance.unwrap_or(0.0);
            c.min_luminance = cc.min_luminance.unwrap_or(0.0);
            c.maxFALL = cc.max_fall.unwrap_or(0.0);
            weston_sys::weston_output_set_color_characteristics(output.as_ptr(), &c);
        }
    }
    true
}

fn enable_head(
    ctx: &Ctx,
    head: NonNull<weston_sys::weston_head>,
    setup: HeadSetup,
    mirror_of: Option<NonNull<weston_sys::weston_output>>,
    color: &crate::output_policy::ColorSetup,
) {
    let compositor = ctx.inner.compositor.get();
    let name = head_name(head);
    let cname = CString::new(name.clone()).unwrap_or_else(|_| c"head".into());

    // SAFETY: compositor + head live (inside the flush).
    let output = unsafe {
        weston_sys::weston_compositor_create_output(compositor, head.as_ptr(), cname.as_ptr())
    };
    let Some(output) = NonNull::new(output) else {
        // C simple_head_enable wording.
        crate::log::log_line(&format!("Could not create an output for head \"{name}\"."));
        ctx.inner.init_failed.set(true);
        return;
    };

    // C simple_head_enable ordering: placement (pre_enable for a
    // mirror, lazy_align otherwise), then the backend configurator
    // (scale/transform/size), then enable.
    match mirror_of {
        Some(src) => {
            // C wet_output_overlap_pre_enable: the backend reads
            // output->mirror_of during configure/enable, and the
            // mirror sits exactly over its source.
            // SAFETY: output just created and not enabled; src is the
            // live output whose creation signal we are inside (or a
            // resolved live output on the resize path).
            unsafe {
                (*output.as_ptr()).mirror_of = src.as_ptr();
                weston_sys::weston_output_set_position(output.as_ptr(), (*src.as_ptr()).pos);
            }
        }
        None => lazy_align(compositor, output),
    }

    // SAFETY: output live and not yet enabled; mirrors the C
    // per-backend configurator with the resolved policy decision
    // (defaults → [[output]] section → CLI).
    let configured = unsafe {
        match &setup {
            // wet_configure_windowed_output_from_config.
            HeadSetup::Windowed(s, which) => {
                weston_sys::weston_output_set_scale(output.as_ptr(), s.scale);
                weston_sys::weston_output_set_transform(output.as_ptr(), s.transform.to_c());
                let api = weston_sys::weston_plugin_api_get(
                    compositor,
                    which.name().as_ptr(),
                    std::mem::size_of::<weston_sys::weston_windowed_output_api>(),
                )
                .cast::<weston_sys::weston_windowed_output_api>();
                !api.is_null()
                    && ((*api).output_set_size.expect("output_set_size"))(
                        output.as_ptr(),
                        s.width,
                        s.height,
                    ) >= 0
            }
            // vnc_backend_output_configure: section scale, transform
            // forced normal, three-arg set_size with resizeable —
            // which a mirror forces off (C: `if (output->mirror_of &&
            // resizeable)`, with its exact log line).
            HeadSetup::Vnc(s) => {
                let mut resizeable = s.resizeable;
                if mirror_of.is_some() && resizeable {
                    resizeable = false;
                    crate::log::log_line(&format!(
                        "Use of mirror_of disables resizing for output {name}"
                    ));
                }
                weston_sys::weston_output_set_scale(output.as_ptr(), s.scale);
                weston_sys::weston_output_set_transform(
                    output.as_ptr(),
                    weston_sys::wl_output_transform::WL_OUTPUT_TRANSFORM_NORMAL,
                );
                let api = weston_sys::weston_plugin_api_get(
                    compositor,
                    weston_sys::WESTON_VNC_OUTPUT_API_NAME.as_ptr().cast(),
                    std::mem::size_of::<weston_sys::weston_vnc_output_api>(),
                )
                .cast::<weston_sys::weston_vnc_output_api>();
                !api.is_null()
                    && ((*api).output_set_size.expect("vnc output_set_size"))(
                        output.as_ptr(),
                        s.width,
                        s.height,
                        resizeable,
                    ) >= 0
            }
            // C pipewire_backend_output_configure (main.c:3558):
            // section scale, transform forced NORMAL, the per-output
            // gbm-format, then output_set_size.
            HeadSetup::Pipewire(s) => {
                weston_sys::weston_output_set_scale(output.as_ptr(), s.scale);
                weston_sys::weston_output_set_transform(
                    output.as_ptr(),
                    weston_sys::wl_output_transform::WL_OUTPUT_TRANSFORM_NORMAL,
                );
                let api = weston_sys::weston_plugin_api_get(
                    compositor,
                    weston_sys::WESTON_PIPEWIRE_OUTPUT_API_NAME.as_ptr().cast(),
                    std::mem::size_of::<weston_sys::weston_pipewire_output_api>(),
                )
                .cast::<weston_sys::weston_pipewire_output_api>();
                if api.is_null() {
                    crate::log::log_line("Cannot use weston_pipewire_output_api.");
                    false
                } else {
                    // C passes NULL when the section has no gbm-format.
                    let fmt = s
                        .gbm_format
                        .as_ref()
                        .and_then(|f| CString::new(f.as_str()).ok());
                    ((*api).set_gbm_format.expect("set_gbm_format"))(
                        output.as_ptr(),
                        fmt.as_ref().map_or(std::ptr::null(), |c| c.as_ptr()),
                    );
                    ((*api).output_set_size.expect("pipewire output_set_size"))(
                        output.as_ptr(),
                        s.width,
                        s.height,
                    ) >= 0
                }
            }
        }
    };
    // C's simple configures call allow_content_protection and
    // wet_output_set_color_profile and stop there — the eotf,
    // colorimetry and characteristics keys are DRM-path only
    // (main.c:1873/1881 vs 2415-2423), hence `full: false`.  The
    // frontend refuses the DRM-only keys on a non-DRM output rather
    // than letting them be inert here.
    let configured = configured && apply_color(ctx, output, color, false);
    if !configured {
        // C wet_configure_windowed_output_from_config failure path.
        crate::log::log_line(&format!("Cannot configure output \"{name}\"."));
        ctx.inner.init_failed.set(true);
        // SAFETY: output was created above and never enabled; it is not
        // registered yet, so no listener fires.
        unsafe { weston_sys::weston_output_destroy(output.as_ptr()) };
        return;
    }

    // SAFETY: output live, configured, not yet enabled.
    let enable_ret = unsafe { weston_sys::weston_output_enable(output.as_ptr()) };
    if enable_ret != 0 {
        // No log line here: libweston's weston_output_enable already
        // emits `Enabling output "%s" failed.`, which is verbatim what
        // C's frontend would add on top of it.
        ctx.inner.init_failed.set(true);
        // SAFETY: output was created above and never enabled; it is not
        // registered yet, so no listener fires.
        unsafe { weston_sys::weston_output_destroy(output.as_ptr()) };
        return;
    }

    // Register head + output with their destroy listeners — insertion
    // and invalidator attachment paired (§3b).  AFTER the enable, as C
    // does with wet_head_tracker_create, and load-bearing for the
    // output side: weston_output_enable emits output_created_signal,
    // and the shell's listener registers the output itself and fires
    // Event::OutputCreated (its background curtain hangs off that).
    // Claiming the registry slot first would make that listener's
    // already-registered early-return swallow the event, and would
    // leave two destroy listeners keyed on one output address.
    register_head(ctx, head, &name);
    register_output(ctx, output, &name);

    // C wet_output_overlap_post_enable: the mirror's final modeline is
    // computed from the source's native mode and applied after enable.
    if let Some(src) = mirror_of {
        apply_mirror_modeline(ctx, &name, src, output);
    }
}

/// C wet_output_compute_output_from_mirror + weston_output_mode_set_
/// native: remote mode = source native mode / remote scale, refresh
/// from the source; the scale argument is the SOURCE's current scale.
///
/// Deliberate divergence (fail-loud upgrade of a C crash, verified
/// live against the oracle): the headless backend publishes no native
/// mode — it points current_mode at its one static mode and never
/// calls weston_output_copy_native_mode, where drm/x11/pipewire do and
/// vnc/rdp get it through weston_output_set_single_mode — so a
/// headless mirror source leaves native_mode_copy zeroed and C aborts
/// on its `assert(output->native_mode_copy.width)` (main.c:2543).  A
/// static output's current mode IS its native mode, so fall back to
/// current_mode; if neither is usable, flag init_failed instead of
/// letting libweston's compositing-area assert kill the process.
///
/// Shared by the enable and the resize path; C computes the same
/// modeline in both but logs it only in _post_enable, so a resize
/// gains one informational line here.
fn apply_mirror_modeline(
    ctx: &Ctx,
    name: &str,
    src: NonNull<weston_sys::weston_output>,
    output: NonNull<weston_sys::weston_output>,
) {
    // SAFETY: both outputs live (inside the creation/resize emission);
    // POD reads (current_mode is a live mode-list entry on an enabled
    // output), then a plain libweston call with a stack mode whose
    // fields mode_set_native copies.
    unsafe {
        let src_ref = &*src.as_ptr();
        let (src_w, src_h, src_refresh) = if src_ref.native_mode_copy.width > 0 {
            (
                src_ref.native_mode_copy.width,
                src_ref.native_mode_copy.height,
                src_ref.native_mode_copy.refresh,
            )
        } else if !src_ref.current_mode.is_null() {
            let m = &*src_ref.current_mode;
            (m.width, m.height, m.refresh)
        } else {
            crate::log::log_line(&format!(
                "mirror-of source '{}' has no usable mode for output '{name}'",
                output_name(src)
            ));
            ctx.inner.init_failed.set(true);
            return;
        };
        let remote_scale = (*output.as_ptr()).current_scale.max(1);
        let mut mode: weston_sys::weston_mode = std::mem::zeroed();
        mode.width = src_w / remote_scale;
        mode.height = src_h / remote_scale;
        mode.refresh = src_refresh;
        let scale = src_ref.current_scale;
        crate::log::log_line(&format!(
            "Setting modeline to output '{name}' to {}x{}, scale: {scale}",
            mode.width, mode.height
        ));
        weston_sys::weston_output_mode_set_native(output.as_ptr(), &mut mode, scale);
    }
}

/// Returns true when this call registered the head, false when it was
/// already known (callers use it as a once-per-head gate).
pub(crate) fn register_head(ctx: &Ctx, head: NonNull<weston_sys::weston_head>, name: &str) -> bool {
    if ctx.inner.heads.borrow().id_of(head).is_some() {
        return false;
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
    true
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
