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
        // C load_headless_backend: the headless default is the no-op
        // renderer unless one was asked for explicitly.
        Renderer::Auto | Renderer::Noop => weston::RendererKind::Noop,
        Renderer::Gl => weston::RendererKind::Gl,
        Renderer::Pixman => weston::RendererKind::Pixman,
    };

    let mut builder = weston::CompositorBuilder::headless()
        .renderer(renderer)
        .with_output_policy(policy);
    if settings.no_outputs {
        builder = builder.with_no_outputs();
    }
    if let Some(mhz) = settings.refresh_rate {
        builder = builder.with_refresh_mhz(mhz);
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

/// R2a fail-loud gate: options whose behavior is not yet ported are
/// startup errors, never silent no-ops (the C frontend stays the
/// oracle for them — plan §7).
fn reject_unported(cli: &Cli, settings: &Settings) -> Option<ExitCode> {
    if settings.backends != [Backend::Headless] {
        let names: Vec<&str> = settings
            .backends
            .iter()
            .filter(|b| **b != Backend::Headless)
            .map(|b| b.name())
            .collect();
        return Some(fatal(&format!(
            "backend \"{}\" is not yet ported to the Rust frontend (R2a supports \
             headless only); use the C westonite",
            names.join("\", \"")
        )));
    }
    if settings.xwayland {
        return Some(fatal(
            "--xwayland is not yet ported to the Rust frontend (lands at R2d); \
             use the C westonite",
        ));
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
        if out.clone_of.is_some() || out.mirror_of.is_some() {
            return Some(fatal(&format!(
                "[[output]] '{name}': clone-of/mirror-of are not yet ported to the Rust \
                 frontend (DRM/remote sharing lands with the DRM slice)"
            )));
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

/// Resolve the settings into the fence's [`weston::OutputPolicy`]
/// (R2b): every `[[output]]` section becomes a typed rule (inert unless
/// a head with that name appears — C's name-matched section lookup),
/// CLI --width/--height/--scale/--transform become the overriding
/// layer.  Bad transform names are startup errors (C
/// wet_output_set_transform fails the enable; we fail earlier with the
/// same wording); a bad mode logs C's "Invalid mode … Using defaults."
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
            scale: None,
            transform: None,
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
        if let Some(s) = out.scale {
            if s <= 0 {
                return Err(format!(
                    "[[output]] '{name}': scale must be positive (got {s})"
                ));
            }
            rule.scale = Some(s);
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
/// the headless output).
fn parse_mode(mode: &str) -> Option<(i32, i32)> {
    let core = mode.split('@').next().unwrap_or(mode);
    let (w, h) = core.split_once('x')?;
    let w: i32 = w.trim().parse().ok()?;
    let h: i32 = h.trim().parse().ok()?;
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
