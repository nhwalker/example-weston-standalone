//! The westonite frontend (plan §4: the main.c port, growing by slice).
//!
//! R2a slice: CLI/config (§5 re-spec — TOML + clap + `-o`), logging,
//! XDG_RUNTIME_DIR verification, the headless backend, the statically
//! linked Rust shell, autolaunch (via `westonite-spawn`), SIGCHLD
//! watch, clean signal-driven shutdown.
//! R2b slice: real output management — `[[output]]` mode/scale/
//! transform/off resolved into an [`weston::OutputPolicy`], CLI
//! `--scale`/`--transform`/`--no-outputs`/`--refresh-rate`, hotplug
//! enable/disable via the policy-driven heads-changed handler.
//! Everything not yet ported fails loudly (never silently degrades) —
//! the C frontend remains the oracle for those paths until their slice
//! lands (plan §7).

#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use westonite_config::{Backend, Cli, Renderer, Settings};

/// A startup-fatal error: logged through weston_log (so it lands in
/// `--log` when one is set, stderr otherwise), then exit(1) — the C
/// frontend's `goto out` contract.
fn fatal(msg: &str) -> ExitCode {
    weston::log::message(&format!("fatal: {msg}"));
    ExitCode::FAILURE
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    if cli.version {
        println!("westonite {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }

    // Logging first, in C's order (main.c:4499): the fallback handlers
    // so the next few lines can fail loudly, then the real log context
    // -- scope, log file, subscribers -- before the banner.
    weston::log::install_stderr_handlers();
    let log = match weston::log::LogContext::new(&weston::log::LogSetup {
        file: cli.log.as_deref().map(std::path::PathBuf::from),
        logger_scopes: split_scopes(cli.logger_scopes.as_deref()),
        flight_rec_scopes: cli
            .flight_rec_scopes
            .as_deref()
            .map(|v| split_scopes(Some(v))),
    }) {
        Ok(l) => l,
        // C exits here too (main.c:4501/4509): with no log context and
        // no log file there is nowhere for the session's diagnostics to
        // go, and starting anyway would hide every later failure.
        Err(e) => return fatal(&e.to_string()),
    };

    weston::log::message(&format!(
        "westonite {} (Rust frontend, weston 14 based)",
        env!("CARGO_PKG_VERSION")
    ));
    let cmdline: Vec<String> = std::env::args().collect();
    weston::log::message(&format!("Command line: {}", cmdline.join(" ")));
    // C main.c:4534, and in the same place: whether the in-memory
    // recorder is running is the first thing you want to know when
    // reading a log from a session that went wrong.
    weston::log::message(&format!(
        "Flight recorder: {}",
        if log.flight_rec_enabled() {
            "enabled"
        } else {
            "disabled"
        }
    ));

    if let Some(code) = verify_xdg_runtime_dir() {
        return code;
    }

    let settings = match westonite_config::resolve(&cli) {
        Ok(s) => s,
        Err(e) => return fatal(&e.to_string()),
    };

    match &settings.config_path {
        Some(p) => weston::log::message(&format!("Using config file '{}'", p.display())),
        None => weston::log::message("Starting with no config file."),
    }
    if let Some(ini) = &settings.legacy_ini_found {
        weston::log::message(&format!(
            "warning: found legacy ini config '{}' and ignored it: westonite reads \
             westonite.toml now (see docs/config-migration.md for the ini mapping)",
            ini.display()
        ));
    }

    if let Some(code) = reject_unported(&cli, &settings) {
        return code;
    }

    // C main.c:4589, and in the same place: after the config is loaded
    // (so `[core] wait-for-debugger` counts) but before anything is
    // created, which is the point of stopping at all.
    if settings.wait_for_debugger {
        weston::wait_for_debugger();
    }

    let policy = match build_output_policy(&settings) {
        Ok(p) => p,
        Err(msg) => return fatal(&msg),
    };
    let input_config = match build_input_config(&settings.config.libinput) {
        Ok(c) => c,
        Err(msg) => return fatal(&msg),
    };
    let renderer = match settings.renderer {
        // C load_backend passes the one global choice, AUTO included,
        // into every loader's config.renderer and lets each backend
        // resolve it (headless → noop, vnc → pixman); the first backend
        // to reach `if (!compositor->renderer)` wins for the whole
        // compositor.  Resolving AUTO here instead would make the
        // result depend on our resolution rather than the load order —
        // `--backends=vnc,headless` would hand the VNC backend
        // WESTON_RENDERER_NOOP, which it rejects outright ("unsupported
        // renderer", vnc.c) where C comes up on pixman.
        Renderer::Auto => weston::RendererKind::Auto,
        Renderer::Noop => weston::RendererKind::Noop,
        Renderer::Gl => weston::RendererKind::Gl,
        Renderer::Pixman => weston::RendererKind::Pixman,
    };

    let kb = &settings.config.keyboard;
    let mut builder = weston::CompositorBuilder::new()
        .with_log_context(&log)
        .renderer(renderer)
        .with_output_policy(policy)
        .with_keyboard(weston::KeyboardConfig {
            rules: kb.keymap_rules.clone(),
            model: kb.keymap_model.clone(),
            layout: kb.keymap_layout.clone(),
            variant: kb.keymap_variant.clone(),
            options: kb.keymap_options.clone(),
            repeat_rate: kb.repeat_rate.and_then(|v| i32::try_from(v).ok()),
            repeat_delay: kb.repeat_delay.and_then(|v| i32::try_from(v).ok()),
            vt_switching: kb.vt_switching,
        });
    if let Some(msec) = settings.config.core.repaint_window {
        builder = builder.with_repaint_window_msec(msec);
    }
    builder = builder.color_management(settings.color_management);
    builder = builder.debug_protocol(settings.debug_protocol);
    if let Some(r) = &settings.config.core.require_outputs {
        match weston::RequireOutputs::parse(r) {
            Some(r) => builder = builder.require_outputs(r),
            None => {
                return fatal(&format!(
                    "[core] require-outputs: unknown value \"{r}\" (any|all|none)"
                ));
            }
        }
    }
    // C load_backends: comma-list order, primary first.
    for b in &settings.backends {
        builder = match b {
            Backend::Headless => builder.add_headless(weston::HeadlessOptions {
                no_outputs: settings.no_outputs,
                refresh_mhz: settings.refresh_rate,
                decorate: settings.config.core.output_decorations,
            }),
            Backend::Vnc => builder.add_vnc(weston::VncOptions {
                bind_address: settings.vnc_bind_address.clone(),
                port: settings.rdp_vnc_port.map(i32::from),
                refresh_rate_hz: settings
                    .vnc_refresh_rate
                    .and_then(|v| i32::try_from(v).ok()),
                tls_cert: settings.vnc_tls_cert.clone(),
                tls_key: settings.vnc_tls_key.clone(),
                disable_tls: settings.vnc_disable_tls,
            }),
            Backend::X11 => builder.add_x11(weston::X11Options {
                fullscreen: settings.fullscreen,
                no_input: settings.no_input,
                output_count: settings
                    .output_count
                    .and_then(|v| i32::try_from(v).ok())
                    .unwrap_or(1),
            }),
            Backend::Wayland => builder.add_wayland(weston::WaylandOptions {
                display_name: settings.wayland_display.clone(),
                fullscreen: settings.fullscreen,
                sprawl: settings.sprawl,
                output_count: settings
                    .output_count
                    .and_then(|v| i32::try_from(v).ok())
                    .unwrap_or(1),
                cursor_theme: settings.cursor_theme.clone(),
                cursor_size: settings.cursor_size,
            }),
            Backend::Pipewire => builder.add_pipewire(weston::PipewireOptions {
                gbm_format: settings.gbm_format.clone(),
                num_outputs: settings
                    .pipewire_num_outputs
                    .and_then(|v| i32::try_from(v).ok())
                    .unwrap_or(1),
            }),
            Backend::Drm => builder.add_drm(weston::DrmOptions {
                seat_id: settings.drm_seat.clone(),
                specific_device: settings.drm_device.clone(),
                additional_devices: settings.drm_additional_devices.clone(),
                gbm_format: settings.gbm_format.clone(),
                pageflip_timeout: settings.config.core.pageflip_timeout.unwrap_or(0),
                use_pixman_shadow: settings.config.core.pixman_shadow.unwrap_or(true),
                current_mode: settings.drm_current_mode,
                continue_without_input: settings.continue_without_input,
                input: input_config.clone(),
            }),
            // reject_unported() has already refused rdp, a permanent
            // product decision — see there.
            _ => builder,
        };
    }
    builder = match &settings.socket {
        Some(name) => builder.with_socket_name(name),
        None => builder.with_socket(),
    };
    builder = builder.with_shell(settings.background_color, |bg| {
        Box::new(westonite_shell::Shell::new(westonite_shell::ShellConfig {
            background_color: bg,
        }))
    });
    // C main.c:4725: --xwayland / [core] xwayland loads the module
    // (lazy server spawn from [xwayland] path).
    if settings.xwayland {
        builder = builder.with_xwayland(settings.xwayland_path.clone());
    }

    let mut compositor = match builder.build() {
        Ok(c) => c,
        Err(e) => return fatal(&e.to_string()),
    };

    if let Some(code) = start_autolaunch(&cli, &settings, &compositor) {
        return code;
    }

    let exit_code = compositor.run();
    drop(compositor);
    match u8::try_from(exit_code) {
        Ok(0) => ExitCode::SUCCESS,
        Ok(n) => ExitCode::from(n),
        Err(_) => ExitCode::FAILURE,
    }
}

/// C main.c verify_xdg_runtime_dir: unset or non-directory is fatal
/// (the message wording is pinned by test_cli), wrong mode *or owner*
/// is a warning.
fn verify_xdg_runtime_dir() -> Option<ExitCode> {
    use std::os::unix::fs::MetadataExt;
    let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") else {
        return Some(fatal(
            "environment variable XDG_RUNTIME_DIR is not set.\n\
             Refer to your distribution on how to get it, or\n\
             http://www.freedesktop.org/wiki/Specifications/basedir-spec\n\
             on how to implement it.",
        ));
    };
    let path = PathBuf::from(&dir);
    let not_a_dir = || {
        Some(fatal(&format!(
            "environment variable XDG_RUNTIME_DIR is set to \"{}\", which is not a directory.",
            path.display()
        )))
    };
    let Ok(meta) = std::fs::metadata(&path) else {
        return not_a_dir();
    };
    if !meta.is_dir() {
        return not_a_dir();
    }
    // C masks with 0777 — the setuid/setgid/sticky bits are not part of
    // the check, and the owner must be us.
    let mode = meta.mode() & 0o777;
    // Through westonite-spawn: the getuid call itself belongs in the
    // audited-unsafe crate, so this one keeps forbid(unsafe_code).
    let uid = westonite_spawn::real_uid();
    if mode != 0o700 || meta.uid() != uid {
        weston::log::message(&format!(
            "warning: XDG_RUNTIME_DIR \"{}\" is not configured correctly: unix access \
             mode must be 0700 (current mode is {mode:04o}), and it must be owned by \
             UID {uid} (current owner is UID {})",
            path.display(),
            meta.uid()
        ));
    }
    None
}

/// Fail-loud gate: options whose behavior is not yet ported are
/// startup errors, never silent no-ops (the C frontend stays the
/// oracle for them — plan §7).
///
/// `[[output]]` sections are checked whether or not a head of that name
/// will show up.  C would leave an unmatched section inert, but "inert"
/// and "honoured" are indistinguishable to someone who wrote
/// `clone-of` and got no output and no message — so an unported key is
/// fatal wherever it appears.  `build_output_policy` validates the
/// ported keys on the same all-sections basis.
fn reject_unported(cli: &Cli, settings: &Settings) -> Option<ExitCode> {
    // RDP is a deliberate product decision, not a gap: it stays
    // unimplemented, so say so in its own words rather than promising a
    // future slice (see PROVENANCE / plan §7).
    if settings.backends.contains(&Backend::Rdp) {
        return Some(fatal(
            "the rdp backend is not supported by westonite (deliberately dropped); \
             use the vnc backend for remote access",
        ));
    }
    // [remote-output] — the remoting plugin — is dropped by the same
    // kind of product decision (2026-07-31).  Not deprecated upstream
    // (weston 15.0.90 still builds it by default), just not something
    // westonite ships: it streams an output over GStreamer/RTP, works
    // only on the DRM backend, and westonite's remote story is VNC.
    //
    // Note this was a SILENT no-op before this refusal existed: the
    // section parsed into the config model and nothing ever read it,
    // which is exactly the failure mode the fail-loud rule exists to
    // prevent.
    if !settings.config.remote_output.is_empty() {
        return Some(fatal(
            "[[remote-output]] is not supported by westonite (the remoting plugin is \
             deliberately dropped); use the vnc backend for remote access",
        ));
    }
    // [pipewire-output] — the pipewire *plugin* — is dropped alongside
    // remoting (2026-07-31).  Weston keeps these as two independent
    // build options and they are genuinely different features:
    //   backend-pipewire  "PipeWire backend: screencasting via PipeWire"
    //   pipewire          "Virtual remote output with Pipewire on DRM backend"
    // The BACKEND is ported (--backend=pipewire, R2c-nested); it is
    // only this DRM-grafted virtual output that westonite does not
    // ship.  Neither is deprecated upstream — both are still default-on
    // in weston 15.0.90 — so this is a product decision, like RDP and
    // remoting, not a reaction to upstream removing anything.
    if !settings.config.pipewire_output.is_empty() {
        return Some(fatal(
            "[[pipewire-output]] is not supported by westonite (the pipewire virtual-output \
             plugin is deliberately dropped); the pipewire *backend* (--backend=pipewire) \
             is supported",
        ));
    }
    // (There used to be a catch-all "backend not yet ported" refusal
    // here.  Every Backend variant is now either loaded or refused by
    // name above -- Rdp returns early with its own message -- so the
    // filter could never match, and a refusal that cannot fire is worse
    // than none: it reads like a live guard.)

    // C consumes CLI options per backend loader; anything left over is
    // `fatal: unhandled option`.  Same contract here, as a table: a
    // flag whose consuming backend is not loaded is a startup error,
    // not a silent no-op.
    let has_headless = settings.backends.contains(&Backend::Headless);
    let has_vnc = settings.backends.contains(&Backend::Vnc);
    let has_x11 = settings.backends.contains(&Backend::X11);
    let has_wayland = settings.backends.contains(&Backend::Wayland);
    let has_pipewire = settings.backends.contains(&Backend::Pipewire);
    let has_drm = settings.backends.contains(&Backend::Drm);
    // One row per flag, with the set of loaded backends that would
    // consume it — transcribed from the C `weston_option` tables:
    // headless main.c:3497, x11 3947, wayland 4070, vnc 3707,
    // pipewire 3625.  (--width/--height are in every table, so they
    // never appear here.)
    let per_backend_flags = [
        (cli.seat.is_some(), "--seat", has_drm),
        (cli.drm_device.is_some(), "--drm-device", has_drm),
        (
            cli.additional_devices.is_some(),
            "--additional-devices",
            has_drm,
        ),
        (cli.current_mode, "--current-mode", has_drm),
        (
            cli.continue_without_input,
            "--continue-without-input",
            has_drm,
        ),
        (
            settings.scale.is_some(),
            "--scale",
            has_headless || has_x11 || has_wayland,
        ),
        (settings.transform.is_some(), "--transform", has_headless),
        (settings.no_outputs, "--no-outputs", has_headless),
        (
            settings.refresh_rate.is_some(),
            "--refresh-rate",
            has_headless,
        ),
        (cli.use_gl, "--use-gl", has_headless),
        (
            cli.use_pixman,
            "--use-pixman",
            has_headless || has_x11 || has_wayland,
        ),
        (cli.fullscreen, "--fullscreen", has_x11 || has_wayland),
        (
            cli.output_count.is_some(),
            "--output-count",
            has_x11 || has_wayland,
        ),
        (cli.no_input, "--no-input", has_x11),
        (cli.sprawl, "--sprawl", has_wayland),
        (cli.display.is_some(), "--display", has_wayland),
        (cli.port.is_some(), "--port", has_vnc),
        (cli.address.is_some(), "--address", has_vnc),
        (cli.vnc_tls_cert.is_some(), "--vnc-tls-cert", has_vnc),
        (cli.vnc_tls_key.is_some(), "--vnc-tls-key", has_vnc),
        (
            cli.disable_transport_layer_security,
            "--disable-transport-layer-security",
            has_vnc,
        ),
    ];
    // `has_pipewire` has no flags of its own (C's pipewire table is
    // width/height only); named so the set stays visibly complete.
    let _ = has_pipewire;
    for (set, flag, consumed_by_a_loaded_backend) in per_backend_flags {
        if set && !consumed_by_a_loaded_backend {
            return Some(unhandled_option(flag));
        }
    }
    // Consumed by no ported backend at all: the drm-only options, and
    // the rdp ones (a backend westonite deliberately does not ship).
    // Reaching here means the flag is left over unconditionally — C's
    // `unhandled option`.
    let unconsumed_flags = [
        (cli.rdp_tls_cert.is_some(), "--rdp-tls-cert"),
        (cli.rdp_tls_key.is_some(), "--rdp-tls-key"),
        (cli.external_listener_fd.is_some(), "--external-listener-fd"),
        (cli.no_resizeable, "--no-resizeable"),
        (cli.rdp4_key.is_some(), "--rdp4-key"),
        (cli.env_socket, "--env-socket"),
        (cli.no_remotefx_codec, "--no-remotefx-codec"),
        (cli.force_no_compression, "--force-no-compression"),
    ];
    for (set, flag) in unconsumed_flags {
        if set {
            return Some(unhandled_option(flag));
        }
    }
    // [libinput] touchscreen-calibrator / calibration-helper --
    // libweston's weston_touch_calibration protocol plus a helper C
    // runs through system() (main.c:1075/1214) -- dropped by product
    // decision (2026-08-01), not deferred.  It is a different feature
    // from the per-device settings around it, and westonite ships no
    // client that could drive it: upstream's weston-touch-calibrator
    // is not in our package, and without a touchscreen there is
    // nothing to test against either.
    //
    // Not gated on DRM: C enables the calibrator from
    // weston_compositor_init_config, whatever the backend.
    if settings
        .config
        .libinput
        .touchscreen_calibrator
        .unwrap_or(false)
    {
        return Some(fatal(
            "[libinput] touchscreen-calibrator is not supported by westonite (the touch \
             calibration protocol is deliberately dropped; no calibrator client ships \
             with it)",
        ));
    }
    if settings
        .config
        .libinput
        .calibration_helper
        .as_deref()
        .is_some_and(|h| !h.is_empty())
    {
        return Some(fatal(
            "[libinput] calibration-helper is only consulted by touchscreen-calibrator, \
             which is not supported by westonite",
        ));
    }
    // C's deprecated `enable_tap` underscore spelling — honored there
    // behind a "!!DEPRECATION WARNING!!" (main.c:2260-2267) — is
    // dropped in the re-spec: one spelling, `enable-tap`.  The key
    // stays in the model so this can say what to type instead of
    // pointing a generic unknown-field error at a spelling the C docs
    // never used (the user typed what their old weston.ini had).
    if settings.config.libinput.enable_tap_deprecated.is_some() {
        return Some(fatal(
            "[libinput] enable_tap is C's deprecated spelling and is not supported by \
             westonite; spell it enable-tap",
        ));
    }
    // `--modules` / `[core] modules` — third-party `wet_module_init`
    // plugins — dropped by product decision (2026-08-01), reversing
    // plan D2's "modules= dlopen survives".  Nothing westonite ships
    // uses it: the shell is linked in (D2), and the two plugins that
    // did load this way (remoting, pipewire-output) are themselves
    // dropped.  What it would cost is the whole reason to say no —
    // a stable `wet_module_init` ABI, a dlopen path through the fence
    // for arbitrary C, and a decision about the `wet_get_config`
    // contract D9 deliberately ends at R3.
    //
    // "For now": the door is not nailed shut, and the key stays in the
    // config model so this message can be a real one instead of a
    // generic unknown-field error.
    if !settings.modules.is_empty() {
        return Some(fatal(
            "--modules / [core] modules is not supported by westonite (third-party \
             plugin loading is deliberately dropped); the shell is built in",
        ));
    }
    // R2b: outputs (mode/scale/transform/off, --no-outputs,
    // --refresh-rate) are ported; the attributes still DRM-bound stay
    // fail-loud so a request for them never silently degrades.
    for out in &settings.config.output {
        let name = out.name.as_deref().unwrap_or("<unnamed>");
        // clone-of is a DRM concept: C only resolves it in
        // drm_config_find_controlling_output_section.  On any other
        // backend the key would be silently inert, so say so.
        if out.clone_of.is_some() && !has_drm {
            return Some(fatal(&format!(
                "[[output]] '{name}': clone-of applies to the drm backend only"
            )));
        }
        // mirror-of (R2c-mirror): valid only on a remote head's section
        // — C's machinery only ever mirrors ONTO rdp/vnc/pipewire
        // outputs (the simple_head_enable deferral is keyed on those
        // backend types), and of them only vnc is ported, whose one
        // head is named "vnc".  On any other section the key would be
        // silently inert (C's lazy sections make it a no-op there;
        // fail-loud instead).
        if let Some(src) = &out.mirror_of {
            if !settings.backends.contains(&Backend::Vnc) {
                return Some(fatal(&format!(
                    "[[output]] '{name}': mirror-of requires a remote backend (vnc) in \
                     the loaded backends"
                )));
            }
            if name != "vnc" {
                return Some(fatal(&format!(
                    "[[output]] '{name}': mirror-of is supported on remote outputs only \
                     (the vnc head)"
                )));
            }
            if src == name {
                return Some(fatal(&format!(
                    "[[output]] '{name}': mirror-of must name a different output"
                )));
            }
        }
        // max-bpc IS ported (R2c-drm reads it in decide_drm, including
        // C's mode=current interaction) — but only the DRM configure
        // consumes it, so elsewhere it would be inert.  Same shape as
        // clone-of above.
        if out.max_bpc.is_some() && !has_drm {
            return Some(fatal(&format!(
                "[[output]] '{name}': max-bpc applies to the drm backend only"
            )));
        }
        // eotf-mode / colorimetry-mode / color-characteristics reach
        // libweston only through the DRM configure (main.c:2417-2423);
        // the windowed and remote ones read icc-profile and allow-hdcp
        // and nothing else.  On a non-DRM output they would be inert,
        // so say so rather than accept them.
        if !has_drm
            && (out.eotf_mode.is_some()
                || out.colorimetry_mode.is_some()
                || out.color_characteristics.is_some())
        {
            return Some(fatal(&format!(
                "[[output]] '{name}': eotf-mode, colorimetry-mode and \
                 color-characteristics apply to the drm backend only"
            )));
        }
        // C returns from wet_output_set_color_profile before even
        // reading icc_profile when the manager is absent, so an
        // icc-profile without color-management=true is silently
        // ignored there.  Refuse instead.
        if out.icc_profile.is_some() && !settings.color_management {
            return Some(fatal(&format!(
                "[[output]] '{name}': icc-profile requires [core] color-management = true"
            )));
        }
    }

    if let Some(shell) = &cli.shell {
        // Parity flag: the Rust frontend's shell is built in; only the
        // default spelling is accepted (D19).
        if shell != "desktop" && shell != "desktop-shell.so" {
            return Some(fatal(&format!(
                "unknown shell \"{shell}\": the Rust frontend ships only the built-in \
                 desktop shell"
            )));
        }
    }
    None
}

/// `--logger-scopes` / `--flight-rec-scopes`: C's comma list.
///
/// Takes a nested Option so the caller can keep the distinction C keeps
/// for the flight recorder: the flag absent (default scopes) versus the
/// flag given empty (recorder off).
fn split_scopes(v: Option<&str>) -> Vec<String> {
    v.unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(String::from)
        .collect()
}

/// C main.c's leftover-argv contract (`fatal: unhandled option: %s`),
/// reached through the applicability tables in `reject_unported`.
fn unhandled_option(flag: &str) -> ExitCode {
    fatal(&format!(
        "unhandled option: {flag} (not consumed by any loaded backend)"
    ))
}

/// Resolve `[libinput]` into the typed config the DRM backend's
/// `configure_device` hook applies per device.
///
/// Where C warns and carries on with the key silently inert — an
/// unknown `accel-profile` or `scroll-method`, a `scroll-button` name
/// libevdev does not know, an `accel-speed` outside -1..=1 — this is a
/// startup error instead: a setting that was asked for and does nothing
/// is exactly the failure mode the migration refuses to reproduce.
/// Capability gating is *not* validated here, because "this touchpad
/// cannot rotate" is a runtime fact about the device, not a mistake in
/// the config; those keys are skipped per device, as in C.
fn build_input_config(li: &westonite_config::Libinput) -> Result<weston::InputConfig, String> {
    let accel_profile = match &li.accel_profile {
        Some(p) => Some(weston::AccelProfile::parse(p).ok_or_else(|| {
            format!(
                "[libinput] '{p}' is not a valid accel-profile. Try one of: {}",
                weston::AccelProfile::NAMES.join(" ")
            )
        })?),
        None => None,
    };
    if let Some(s) = li.accel_speed
        && !(-1.0..=1.0).contains(&s)
    {
        return Err(format!(
            "[libinput] accel-speed {s} is out of range (-1.0 to 1.0)"
        ));
    }
    let scroll_method = match &li.scroll_method {
        Some(m) => Some(weston::ScrollMethod::parse(m).ok_or_else(|| {
            format!(
                "[libinput] '{m}' is not a valid scroll-method. Try one of: {}",
                weston::ScrollMethod::NAMES.join(" ")
            )
        })?),
        None => None,
    };
    let scroll_button = match &li.scroll_button {
        Some(b) => Some(weston::ScrollButton::parse(b).ok_or_else(|| {
            format!("[libinput] '{b}' is not an evdev button name (e.g. BTN_RIGHT)")
        })?),
        None => None,
    };
    // C reads scroll-button only under scroll-method=button and
    // discards it otherwise; say so rather than accepting it inert.
    if scroll_button.is_some() && scroll_method != Some(weston::ScrollMethod::Button) {
        return Err(
            "[libinput] scroll-button only applies with scroll-method = \"button\"".to_string(),
        );
    }
    Ok(weston::InputConfig {
        enable_tap: li.enable_tap,
        tap_and_drag: li.tap_and_drag,
        tap_and_drag_lock: li.tap_and_drag_lock,
        disable_while_typing: li.disable_while_typing,
        middle_button_emulation: li.middle_button_emulation,
        left_handed: li.left_handed,
        rotation: li.rotation,
        accel_profile,
        accel_speed: li.accel_speed,
        natural_scroll: li.natural_scroll,
        scroll_method,
        scroll_button,
    })
}

/// The colour slice of one `[[output]]`, with C's validation.
///
/// C `parse_color_characteristics` (main.c:1538) enforces a rule that
/// is easy to miss and easy to get wrong: the eleven characteristic
/// keys form five groups — primaries (six values), white (two), max
/// luminance, min luminance, maxFALL — and each group must be given
/// **entirely or not at all**.  Half a group is a config error, not a
/// partial application.  Ranges are checked too: 0..=1 for the
/// chromaticity coordinates, 0..=1e5 for the luminances.
fn build_color_setup(
    settings: &Settings,
    out: &westonite_config::Output,
    name: &str,
) -> Result<weston::ColorSetup, String> {
    let eotf_mode = match &out.eotf_mode {
        Some(m) => Some(weston::EotfMode::parse(m).ok_or_else(|| {
            format!(
                "[[output]] '{name}': '{m}' is not a valid EOTF mode. Try one of: {}",
                weston::EotfMode::NAMES.join(" ")
            )
        })?),
        None => None,
    };
    let colorimetry_mode = match &out.colorimetry_mode {
        Some(m) => Some(weston::ColorimetryMode::parse(m).ok_or_else(|| {
            format!(
                "[[output]] '{name}': '{m}' is not a valid colorimetry mode. Try one of: {}",
                weston::ColorimetryMode::NAMES.join(" ")
            )
        })?),
        None => None,
    };
    // C: a non-default eotf/colorimetry mode needs the colour manager.
    if eotf_mode.is_some_and(|m| m != weston::EotfMode::Sdr) && !settings.color_management {
        return Err(format!(
            "[[output]] '{name}': a non-SDR eotf-mode requires [core] color-management = true"
        ));
    }
    if colorimetry_mode.is_some_and(|m| m != weston::ColorimetryMode::Default)
        && !settings.color_management
    {
        return Err(format!(
            "[[output]] '{name}': a non-default colorimetry-mode requires \
             [core] color-management = true"
        ));
    }

    let characteristics = match &out.color_characteristics {
        Some(cc_name) => Some(resolve_characteristics(settings, cc_name, name)?),
        None => None,
    };

    Ok(weston::ColorSetup {
        icc_profile: out.icc_profile.clone(),
        eotf_mode,
        colorimetry_mode,
        characteristics,
        // C allow_content_protection default (main.c:1681).
        allow_hdcp: out.allow_hdcp.unwrap_or(true),
    })
}

fn resolve_characteristics(
    settings: &Settings,
    cc_name: &str,
    output: &str,
) -> Result<weston::ColorCharacteristics, String> {
    let cc = settings
        .config
        .color_characteristics
        .iter()
        .find(|c| c.name.as_deref() == Some(cc_name))
        .ok_or_else(|| {
            format!(
                "output {output}: no [[color-characteristics]] section with \
                 'name = \"{cc_name}\"' found"
            )
        })?;
    // C rejects ':' in the name: it is reserved for the profile
    // descriptions libweston builds from these.
    if cc_name.contains(':') {
        return Err(format!(
            "[[color-characteristics]] name={cc_name}: reserved name. \
             Do not use ':' character in the name."
        ));
    }

    let range = |v: Option<f64>, key: &str, lo: f64, hi: f64| -> Result<Option<f32>, String> {
        match v {
            // NaN shall not pass, as C puts it.
            Some(x) if x.is_nan() || x < lo || x > hi => Err(format!(
                "[[color-characteristics]] name={cc_name}: {key} value {x} is outside \
                 of the range {lo} - {hi}."
            )),
            Some(x) => Ok(Some(x as f32)),
            None => Ok(None),
        }
    };
    let prim = [
        ("red-x", cc.red_x),
        ("red-y", cc.red_y),
        ("green-x", cc.green_x),
        ("green-y", cc.green_y),
        ("blue-x", cc.blue_x),
        ("blue-y", cc.blue_y),
    ];
    let white = [("white-x", cc.white_x), ("white-y", cc.white_y)];

    // Each group entirely or not at all.
    let group = |keys: &[(&str, Option<f64>)], label: &str| -> Result<bool, String> {
        let set = keys.iter().filter(|(_, v)| v.is_some()).count();
        if set == 0 {
            return Ok(false);
        }
        if set != keys.len() {
            let missing: Vec<&str> = keys
                .iter()
                .filter(|(_, v)| v.is_none())
                .map(|(k, _)| *k)
                .collect();
            return Err(format!(
                "[[color-characteristics]] name={cc_name}: group {label} key {} is missing. \
                 You must set either none or all keys of a group.",
                missing.join(", ")
            ));
        }
        Ok(true)
    };
    let have_primaries = group(&prim, "primaries")?;
    let have_white = group(&white, "white")?;

    let mut out_cc = weston::ColorCharacteristics::default();
    if have_primaries {
        let v: Vec<f32> = prim
            .iter()
            .map(|(k, v)| range(*v, k, 0.0, 1.0).map(|x| x.unwrap_or(0.0)))
            .collect::<Result<_, _>>()?;
        out_cc.primaries = Some([(v[0], v[1]), (v[2], v[3]), (v[4], v[5])]);
    }
    if have_white {
        let x = range(cc.white_x, "white-x", 0.0, 1.0)?.unwrap_or(0.0);
        let y = range(cc.white_y, "white-y", 0.0, 1.0)?.unwrap_or(0.0);
        out_cc.white = Some((x, y));
    }
    out_cc.max_luminance = range(cc.max_luminance, "max-luminance", 0.0, 1e5)?;
    out_cc.min_luminance = range(cc.min_luminance, "min-luminance", 0.0, 1e5)?;
    out_cc.max_fall = range(cc.max_fall, "max-fall", 0.0, 1e5)?;
    Ok(out_cc)
}

/// Resolve the settings into the fence's [`weston::OutputPolicy`]
/// (R2b): every `[[output]]` section becomes a typed rule applied to
/// the head of that name, and CLI --width/--height/--scale/--transform
/// become the overriding layer.  Precedence per head is C's
/// (wet_configure_windowed_output_from_config): backend defaults →
/// name-matched section → CLI.
///
/// Three deliberate divergences from C (see also `reject_unported`,
/// which validates *all* sections for the same fail-loud reason):
///
///  * C resolves a section only when a head of that name shows up, so a
///    section that matches nothing is silently inert — including one
///    with a typo'd name or an unparseable `transform`.  We validate
///    every section at startup instead: a bad transform name is fatal
///    with C's `Invalid transform "…"` wording, and a section with no
///    `name` key (which could never match anything) is fatal too.
///  * C logs `Invalid mode for output %s. Using defaults.` when a
///    section exists but has *no* `mode` key at all
///    (`if (!mode || sscanf(…) < 2)`, main.c parse_simple_mode).  We
///    log it only for a `mode` that is present and unparseable —
///    warning about a section that merely sets `scale` is noise.
///  * With more than one backend, C loses the CLI geometry entirely:
///    every loader calls `wet_init_parsed_options`, which *replaces*
///    `compositor->parsed_options` with a freshly zeroed one (leaking
///    the previous), while `parse_options` has already removed
///    `--width`/`--height`/`--scale`/`--transform` from argv for the
///    first loader that listed them.  The configure callbacks run at
///    the heads flush, after every load, so they all read the *last*
///    loader's empty table — `--backends=headless,vnc --width=800`
///    sizes neither output in C.  We apply the CLI layer to every
///    backend instead (the C option's evident intent), so a
///    multi-backend run honours `--width` where C silently drops it.
fn build_output_policy(settings: &Settings) -> Result<weston::OutputPolicy, String> {
    let mut policy = weston::OutputPolicy::defaults(1024, 640);

    for out in &settings.config.output {
        let Some(name) = out.name.clone() else {
            return Err("[[output]] section without a name= key".to_string());
        };
        let mut rule = weston::OutputRule {
            name: name.clone(),
            off: out.off == Some(true),
            size: None,
            // Positivity is validated in westonite-config, next to the
            // --width/--height/--scale checks it shares a rule with.
            scale: out.scale,
            transform: None,
            resizeable: out.resizeable,
            gbm_format: out.gbm_format.clone(),
            mirror_of: out.mirror_of.clone(),
            // DRM-only keys; the DRM configure is the only reader.
            // `mode_string` keeps the raw text alongside the parsed
            // size because on DRM "preferred"/"current"/a modeline are
            // all valid and only the backend can resolve a modeline.
            mode_string: out.mode.clone(),
            max_bpc: out.max_bpc,
            content_type: out.content_type.clone(),
            seat: out.seat.clone(),
            force_on: out.force_on == Some(true),
            clone_of: out.clone_of.clone(),
            color: build_color_setup(settings, out, &name)?,
        };
        if let Some(mode) = &out.mode {
            if mode == "off" {
                rule.off = true;
            } else if let Some(size) = parse_mode(mode) {
                rule.size = Some(size);
            } else if !matches!(mode.as_str(), "preferred" | "current") {
                // On DRM these two, and anything else, are meaningful
                // (see mode_string above); on every other backend C
                // logs this and falls back to the defaults.
                weston::log::message(&format!("Invalid mode for output {name}. Using defaults."));
            }
        }
        if let Some(t) = &out.transform {
            rule.transform = Some(
                weston::OutputTransform::parse(t)
                    .ok_or_else(|| format!("Invalid transform \"{t}\" for output {name}"))?,
            );
        }
        policy.rules.push(rule);
    }

    policy.cli.width = settings.width;
    policy.cli.height = settings.height;
    policy.cli.scale = settings.scale;
    if let Some(t) = &settings.transform {
        policy.cli.transform = Some(
            weston::OutputTransform::parse(t)
                .ok_or_else(|| format!("Invalid transform \"{t}\""))?,
        );
    }
    Ok(policy)
}

/// "WxH" or "WxH@rate" (weston's simple-mode grammar; rate ignored by
/// the headless output).  Anything else — `preferred`, `current`, a
/// full modeline — is None, and the caller falls back to the defaults
/// with C's log line, exactly as C's `sscanf("%dx%d") != 2` does
/// (the trimmed tree's parse_simple_mode dropped upstream's `@%d`
/// framerate conversion; a trailing `@rate` still parses because
/// sscanf stops after its two conversions).
///
/// Stricter than that sscanf on three shapes it would wave through:
/// embedded spaces (`1024 x 640`), trailing junk (`1024x640junk`,
/// where sscanf stops happily after two conversions), and
/// non-positive dimensions (`-100x480`, `0x0` — C hands them straight
/// to output_set_size).  All are typos, and C silently running at
/// 1024x640 is what makes them expensive.
fn parse_mode(mode: &str) -> Option<(i32, i32)> {
    let core = mode.split('@').next().unwrap_or(mode);
    let (w, h) = core.split_once('x')?;
    let w: i32 = w.parse().ok()?;
    let h: i32 = h.parse().ok()?;
    if w > 0 && h > 0 { Some((w, h)) } else { None }
}

/// C execute_autolaunch / execute_command: X_OK precheck for the config
/// path (exact message pinned by test_children), WAYLAND_DISPLAY from
/// the bound socket, watch registration for the SIGCHLD handler.
fn start_autolaunch(
    cli: &Cli,
    settings: &Settings,
    compositor: &weston::Compositor,
) -> Option<ExitCode> {
    let Some(argv) = &settings.autolaunch else {
        return None;
    };
    let from_config = cli.autolaunch.is_empty();
    if from_config {
        // C: access(path, X_OK); the positional path goes straight to
        // exec (PATH lookup) like execvp does.
        let path = std::path::Path::new(&argv[0]);
        if !is_executable(path) {
            return Some(fatal(&format!(
                "Specified autolaunch path ({}) is not executable",
                argv[0]
            )));
        }
    }
    let Some(mut cmd) = westonite_spawn::Command::from_argv(argv) else {
        return Some(fatal("autolaunch command is empty"));
    };
    if let Some(socket) = compositor.socket_name() {
        cmd = cmd.env("WAYLAND_DISPLAY", socket);
    }
    // D12: no WESTON_CONFIG_FILE export — the TOML config is not
    // readable by any stock client, and we ship none that read it.
    match cmd.spawn() {
        Ok(child) => {
            let pid = match i32::try_from(child.id()) {
                Ok(p) => p,
                Err(_) => return Some(fatal("autolaunch pid out of range")),
            };
            compositor.set_autolaunch(pid, settings.autolaunch_watch);
            None
        }
        Err(e) => Some(fatal(&format!(
            "Failed to spawn the autolaunch process: {e}"
        ))),
    }
}

fn is_executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match std::fs::metadata(path) {
        // access(X_OK) approximation: any execute bit (we run as one
        // uid; exactness beyond this doesn't change the exec outcome —
        // a wrong positive still fails at spawn with a logged error).
        Ok(m) => m.is_file() && m.mode() & 0o111 != 0,
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::parse_mode;

    #[test]
    fn simple_mode_grammar() {
        assert_eq!(parse_mode("1024x640"), Some((1024, 640)));
        assert_eq!(parse_mode("1920x1080@60"), Some((1920, 1080)));
        // Rate present but empty: C's sscanf takes the two conversions
        // it got and moves on, and so do we.
        assert_eq!(parse_mode("800x500@"), Some((800, 500)));

        // Fall back to the defaults (with C's log line) for everything
        // the windowed grammar does not cover.
        for bad in [
            "preferred",
            "current",
            "off",
            "1024",
            "1024x",
            "x640",
            "0x640",
            "1024x-1",
            // Stricter than C's sscanf on purpose — see parse_mode.
            "1024 x 640",
            "1024x640junk",
        ] {
            assert_eq!(parse_mode(bad), None, "{bad}");
        }
    }
}
