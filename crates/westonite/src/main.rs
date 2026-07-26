//! The westonite frontend (plan §4: the main.c port, growing by slice).
//!
//! R2a slice: CLI/config (§5 re-spec — TOML + clap + `-o`), logging,
//! XDG_RUNTIME_DIR verification, the headless backend, the statically
//! linked Rust shell, autolaunch (via `westonite-spawn`), SIGCHLD
//! watch, clean signal-driven shutdown.  Everything not yet ported
//! fails loudly (never silently degrades) — the C frontend remains the
//! oracle for those paths until their slice lands (plan §7).

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

    let (width, height) = headless_geometry(&settings);
    let renderer = match settings.renderer {
        // C load_headless_backend: the headless default is the no-op
        // renderer unless one was asked for explicitly.
        Renderer::Auto | Renderer::Noop => weston::RendererKind::Noop,
        Renderer::Gl => weston::RendererKind::Gl,
        Renderer::Pixman => weston::RendererKind::Pixman,
    };

    let mut builder = weston::CompositorBuilder::headless()
        .renderer(renderer)
        .output_size(width, height);
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
    if settings.no_outputs {
        return Some(fatal(
            "--no-outputs is not yet ported to the Rust frontend (lands at R2b)",
        ));
    }
    // Output attributes the R2a bring-up cannot honour: it hands the
    // fence a single width/height and nothing else, so accepting these
    // silently would degrade rather than fail (plan §7).
    if settings.scale.is_some() {
        return Some(fatal(
            "--scale is not yet ported to the Rust frontend (lands at R2b)",
        ));
    }
    if settings.refresh_rate.is_some() {
        return Some(fatal(
            "--refresh-rate is not yet ported to the Rust frontend (lands at R2b)",
        ));
    }
    // Only the section that applies to this run's head, as C does
    // (weston_config_get_section("output", "name", output->name)):
    // sections for other heads are inert here, exactly as in C.
    if let Some(out) = headless_output_section(settings) {
        if out.off == Some(true) || out.mode.as_deref() == Some("off") {
            return Some(fatal(
                "[[output]] 'headless': disabling an output is not yet ported to the Rust \
                 frontend (lands at R2b with --no-outputs)",
            ));
        }
        if out.scale.is_some() || out.transform.is_some() {
            return Some(fatal(
                "[[output]] 'headless': scale/transform are not yet ported to the Rust \
                 frontend (lands at R2b)",
            ));
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

/// The `[[output]]` block that configures this run's headless head, if
/// the config has one.
fn headless_output_section(settings: &Settings) -> Option<&westonite_config::Output> {
    settings
        .config
        .output
        .iter()
        .find(|o| o.name.as_deref() == Some("headless"))
}

/// Headless output geometry, C precedence (main.c wet_configure_windowed
/// _output_from_config): defaults 1024x640 → `[[output]]` mode= for the
/// "headless" head → CLI --width/--height.
fn headless_geometry(settings: &Settings) -> (i32, i32) {
    let (mut width, mut height) = (1024, 640);
    if let Some(out) = headless_output_section(settings)
        && let Some(mode) = &out.mode
    {
        if let Some((w, h)) = parse_mode(mode) {
            width = w;
            height = h;
        } else {
            weston::log::message(&format!(
                "Invalid mode for output headless. Using defaults. (mode '{mode}')"
            ));
        }
    }
    if let Some(w) = settings.width {
        width = w;
    }
    if let Some(h) = settings.height {
        height = h;
    }
    (width, height)
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
