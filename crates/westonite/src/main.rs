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

    // Logging first: handlers, then the --log file, so every later
    // failure (config errors included) reaches the right sink.
    weston::log::install_stderr_handlers();
    if let Some(path) = &cli.log
        && let Err(e) = weston::log::set_log_file(std::path::Path::new(path))
    {
        // C weston_log_file_open falls back to stderr on failure.
        eprintln!("westonite: cannot open log file '{path}': {e}; logging to stderr");
    }

    weston::log::message(&format!(
        "westonite {} (Rust frontend, weston 14 based)",
        env!("CARGO_PKG_VERSION")
    ));
    let cmdline: Vec<String> = std::env::args().collect();
    weston::log::message(&format!("Command line: {}", cmdline.join(" ")));

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

    let policy = match build_output_policy(&settings) {
        Ok(p) => p,
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
    // C load_backends: comma-list order, primary first.
    for b in &settings.backends {
        builder = match b {
            Backend::Headless => builder.add_headless(weston::HeadlessOptions {
                no_outputs: settings.no_outputs,
                refresh_mhz: settings.refresh_rate,
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
            // reject_unported() has already refused everything else
            // (drm; rdp is a permanent product decision — see there).
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
    let unported: Vec<&str> = settings
        .backends
        .iter()
        .filter(|b| {
            !matches!(
                b,
                Backend::Headless
                    | Backend::Vnc
                    | Backend::X11
                    | Backend::Wayland
                    | Backend::Pipewire
            )
        })
        .map(|b| b.name())
        .collect();
    if !unported.is_empty() {
        return Some(fatal(&format!(
            "backend \"{}\" is not yet ported to the Rust frontend (drm lands with its \
             own slice); use the C westonite",
            unported.join("\", \"")
        )));
    }

    // C consumes CLI options per backend loader; anything left over is
    // `fatal: unhandled option`.  Same contract here, as a table: a
    // flag whose consuming backend is not loaded is a startup error,
    // not a silent no-op.
    let has_headless = settings.backends.contains(&Backend::Headless);
    let has_vnc = settings.backends.contains(&Backend::Vnc);
    let has_x11 = settings.backends.contains(&Backend::X11);
    let has_wayland = settings.backends.contains(&Backend::Wayland);
    let has_pipewire = settings.backends.contains(&Backend::Pipewire);
    // One row per flag, with the set of loaded backends that would
    // consume it — transcribed from the C `weston_option` tables:
    // headless main.c:3497, x11 3947, wayland 4070, vnc 3707,
    // pipewire 3625.  (--width/--height are in every table, so they
    // never appear here.)
    let per_backend_flags = [
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
        (cli.seat.is_some(), "--seat"),
        (cli.drm_device.is_some(), "--drm-device"),
        (cli.additional_devices.is_some(), "--additional-devices"),
        (cli.current_mode, "--current-mode"),
        (cli.continue_without_input, "--continue-without-input"),
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
    if !settings.modules.is_empty() {
        return Some(fatal(
            "--modules is not yet ported to the Rust frontend; use the C westonite",
        ));
    }
    if settings.debug_protocol {
        return Some(fatal(
            "--debug is not yet ported to the Rust frontend; use the C westonite",
        ));
    }
    // R2b: outputs (mode/scale/transform/off, --no-outputs,
    // --refresh-rate) are ported; the attributes still DRM-bound stay
    // fail-loud so a request for them never silently degrades.
    for out in &settings.config.output {
        let name = out.name.as_deref().unwrap_or("<unnamed>");
        if out.clone_of.is_some() {
            return Some(fatal(&format!(
                "[[output]] '{name}': clone-of is not yet ported to the Rust frontend \
                 (same-CRTC clones land with the DRM slice)"
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
        if out.icc_profile.is_some()
            || out.eotf_mode.is_some()
            || out.colorimetry_mode.is_some()
            || out.color_characteristics.is_some()
            || out.max_bpc.is_some()
            || out.vrr_mode.is_some()
        {
            return Some(fatal(&format!(
                "[[output]] '{name}': color-management/DRM attributes are not yet ported \
                 to the Rust frontend"
            )));
        }
    }
    if settings.color_management {
        return Some(fatal(
            "color-management is not yet ported to the Rust frontend",
        ));
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
    if !settings.logger_scopes.is_empty() || !settings.flight_rec_scopes.is_empty() {
        weston::log::message(
            "warning: log scope selection is not yet ported to the Rust frontend; ignoring",
        );
    }
    if settings.wait_for_debugger {
        weston::log::message(
            "warning: --wait-for-debugger is not yet ported to the Rust frontend; ignoring",
        );
    }
    None
}

/// C main.c's leftover-argv contract (`fatal: unhandled option: %s`),
/// reached through the applicability tables in `reject_unported`.
fn unhandled_option(flag: &str) -> ExitCode {
    fatal(&format!(
        "unhandled option: {flag} (not consumed by any loaded backend)"
    ))
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
        };
        if let Some(mode) = &out.mode {
            if mode == "off" {
                rule.off = true;
            } else if let Some(size) = parse_mode(mode) {
                rule.size = Some(size);
            } else {
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
/// Stricter than that sscanf on two shapes it would wave through:
/// embedded spaces (`1024 x 640`) and trailing junk (`1024x640junk`,
/// where sscanf stops happily after two conversions).  Both are typos,
/// and C silently running at 1024x640 is what makes them expensive.
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
