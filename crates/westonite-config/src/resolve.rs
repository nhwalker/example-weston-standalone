//! Resolution (D9–D12): defaults → file → `-o` overrides → flags, into
//! an immutable [`Settings`] resolved once at startup.
//!
//! File discovery mirrors the ini search the C frontend had (P2), with
//! the new name: `$XDG_CONFIG_HOME/westonite.toml`, then
//! `$HOME/.config/westonite.toml`, then each of `$XDG_CONFIG_DIRS`
//! (default `/etc/xdg`).  Both home locations are tried, in that order
//! — libweston's `open_config_file` falls through from
//! `$XDG_CONFIG_HOME` to `$HOME/.config` rather than treating the
//! former as an override, and the C frontend inherits that.
//! `WESTON_CONFIG_FILE` is dropped (D12).  If a legacy
//! `westonite.ini` sits where the TOML is expected, resolution reports
//! a one-line hint and otherwise ignores it (D11).

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

use crate::cli::Cli;
use crate::model::Config;
use crate::overrides;

#[derive(Debug)]
pub enum ConfigError {
    /// File read/parse/validation problems — fatal at startup, with
    /// the TOML span in the message (deny_unknown_fields, D9).
    Invalid(String),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Invalid(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for ConfigError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Backend {
    Drm,
    Headless,
    X11,
    Wayland,
    Rdp,
    Vnc,
    Pipewire,
}

impl Backend {
    pub fn parse(s: &str) -> Option<Backend> {
        // Accept both the short name and the C module-name spelling.
        let s = s.strip_suffix("-backend.so").unwrap_or(s);
        Some(match s {
            "drm" => Backend::Drm,
            "headless" => Backend::Headless,
            "x11" => Backend::X11,
            "wayland" => Backend::Wayland,
            "rdp" => Backend::Rdp,
            "vnc" => Backend::Vnc,
            "pipewire" => Backend::Pipewire,
            _ => return None,
        })
    }

    pub fn name(&self) -> &'static str {
        match self {
            Backend::Drm => "drm",
            Backend::Headless => "headless",
            Backend::X11 => "x11",
            Backend::Wayland => "wayland",
            Backend::Rdp => "rdp",
            Backend::Vnc => "vnc",
            Backend::Pipewire => "pipewire",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Renderer {
    #[default]
    Auto,
    Gl,
    Pixman,
    Noop,
}

/// The immutable result of resolution.  Consumers receive typed slices
/// of this; nothing re-reads the file or CLI afterwards (§5).
#[derive(Debug, Clone)]
pub struct Settings {
    /// Backends to load, primary first (multi-backend via --backends).
    pub backends: Vec<Backend>,
    pub renderer: Renderer,
    pub socket: Option<String>,
    pub log_file: Option<PathBuf>,
    pub debug_protocol: bool,

    pub wait_for_debugger: bool,
    pub xwayland: bool,
    pub idle_time: Option<u32>,
    pub modules: Vec<String>,
    pub require_input: bool,
    pub color_management: bool,
    pub gbm_format: Option<String>,

    /// Parsed `[shell] background-color` (0xAARRGGBB), C default.
    pub background_color: u32,
    pub shell_client: Option<String>,

    /// Windowed/headless geometry (CLI-level; per-output config in
    /// `config.output`).
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub scale: Option<i32>,
    /// `--transform` (weston transform-name grammar, validated by the
    /// frontend against the typed enum).
    pub transform: Option<String>,
    pub fullscreen: bool,
    pub output_count: Option<u32>,
    pub no_input: bool,
    pub sprawl: bool,
    /// `--display`: the parent compositor socket for the nested
    /// wayland backend (None = the child's own WAYLAND_DISPLAY).
    pub wayland_display: Option<String>,
    /// `[shell] cursor-theme` / `cursor-size` (wayland backend).
    pub cursor_theme: Option<String>,
    pub cursor_size: i32,
    /// `[pipewire] num-outputs`, C default 1.
    pub pipewire_num_outputs: Option<u32>,
    pub parent_display: Option<String>,
    pub no_outputs: bool,
    pub refresh_rate: Option<i32>,

    pub drm_seat: Option<String>,
    pub drm_device: Option<String>,
    pub drm_additional_devices: Option<String>,
    pub drm_current_mode: bool,
    pub continue_without_input: bool,

    /// Listen port for whichever of rdp/vnc is in `backends` (`--port`
    /// wins over the matching section).
    pub rdp_vnc_port: Option<u16>,
    /// `--address` / `[vnc] address` (vnc bind address).
    pub vnc_bind_address: Option<String>,
    /// `[vnc] refresh-rate`, Hz (no CLI flag in C either).
    pub vnc_refresh_rate: Option<u32>,
    pub vnc_disable_tls: bool,
    pub rdp_tls_cert: Option<String>,
    pub rdp_tls_key: Option<String>,
    pub vnc_tls_cert: Option<String>,
    pub vnc_tls_key: Option<String>,
    pub rdp_external_listener_fd: Option<i32>,
    /// C config field `resizeable` (CLI --no-resizeable inverts it).
    pub rdp_resizeable: bool,
    pub rdp_force_no_compression: bool,
    pub rdp4_key: Option<String>,
    pub rdp_env_socket: bool,
    /// C default true; CLI --no-remotefx-codec inverts.
    pub rdp_remotefx_codec: bool,

    /// Autolaunch command: CLI trailing args win over `[autolaunch]
    /// path`; watch only from the config.
    pub autolaunch: Option<Vec<String>>,
    pub autolaunch_watch: bool,
    pub xwayland_path: PathBuf,

    /// The config file actually used, if any (logged at startup).
    pub config_path: Option<PathBuf>,
    /// D11 hint: a legacy ini was found and ignored.
    pub legacy_ini_found: Option<PathBuf>,

    /// The full validated model, for consumers of structured sections
    /// (outputs, keyboard, libinput, backend sections).
    pub config: Config,
}

fn parse_color(s: &str, what: &str) -> Result<u32, ConfigError> {
    let t = s.trim();
    let parsed = if let Some(hex) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        u32::from_str_radix(hex, 16)
    } else {
        t.parse::<u32>()
    };
    parsed.map_err(|_| ConfigError::Invalid(format!("{what}: invalid color '{s}'")))
}

fn split_list(s: &str) -> Vec<String> {
    s.split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(String::from)
        .collect()
}

/// File discovery (P2 search order, TOML name).  Returns the chosen
/// path plus any ignored legacy ini next to the search locations.
fn discover(env: &HashMap<String, String>) -> (Option<PathBuf>, Option<PathBuf>) {
    let mut dirs: Vec<PathBuf> = Vec::new();
    // Both home locations, in order — not either/or: libweston tries
    // $XDG_CONFIG_HOME and then falls through to $HOME/.config.
    if let Some(x) = env.get("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        dirs.push(PathBuf::from(x));
    }
    if let Some(h) = env.get("HOME").filter(|v| !v.is_empty()) {
        let home_config = Path::new(h).join(".config");
        if !dirs.contains(&home_config) {
            dirs.push(home_config);
        }
    }
    // Deliberate divergence, documented in docs/config-migration.md:
    // libweston reads the ini from a hard-coded `weston/` SUBDIR of
    // each entry (/etc/xdg/weston/weston.ini); the TOML lives in the
    // entry itself (/etc/xdg/westonite.toml) — no foreign project name
    // in a rebranded compositor's path.
    match env.get("XDG_CONFIG_DIRS").filter(|v| !v.is_empty()) {
        Some(list) => {
            for d in list.split(':').filter(|d| !d.is_empty()) {
                dirs.push(PathBuf::from(d));
            }
        }
        None => dirs.push(PathBuf::from("/etc/xdg")),
    }

    let mut legacy = None;
    for d in &dirs {
        let toml = d.join("westonite.toml");
        if toml.is_file() {
            return (Some(toml), legacy);
        }
        if legacy.is_none() {
            let ini = d.join("westonite.ini");
            if ini.is_file() {
                legacy = Some(ini);
            }
        }
    }
    (None, legacy)
}

/// Resolve from real process environment.
pub fn resolve(cli: &Cli) -> Result<Settings, ConfigError> {
    let env: HashMap<String, String> = std::env::vars().collect();
    resolve_from(cli, &env)
}

/// Resolve with an explicit environment (unit tests).
pub fn resolve_from(cli: &Cli, env: &HashMap<String, String>) -> Result<Settings, ConfigError> {
    // -- file --
    let mut legacy_ini_found = None;
    let config_path: Option<PathBuf> = if cli.no_config {
        None
    } else if let Some(p) = &cli.config {
        Some(PathBuf::from(p))
    } else {
        let (found, legacy) = discover(env);
        legacy_ini_found = legacy;
        found
    };

    let mut tree: toml::Table = match &config_path {
        Some(p) => {
            let text = std::fs::read_to_string(p).map_err(|e| {
                ConfigError::Invalid(format!("cannot read config file '{}': {e}", p.display()))
            })?;
            text.parse()
                .map_err(|e| ConfigError::Invalid(format!("config file '{}': {e}", p.display())))?
        }
        None => toml::Table::new(),
    };

    // -- -o overrides --
    for spec in &cli.set {
        overrides::apply(&mut tree, spec).map_err(ConfigError::Invalid)?;
    }

    // -- deserialize (unknown keys/type errors become startup errors) --
    let config: Config = toml::Value::Table(tree)
        .try_into()
        .map_err(|e| match &config_path {
            Some(p) => ConfigError::Invalid(format!("config file '{}': {e}", p.display())),
            None => ConfigError::Invalid(format!("config overrides: {e}")),
        })?;

    // -- flags on top --
    // --backend/--backends are one C variable (main.c:4458-4459): the
    // last occurrence won and either spelling takes a comma list, so
    // the merged clap field is always split.
    // The two section keys are that same one variable (main.c:4602-6):
    // `backends` is consulted first and `backend` only as its fallback,
    // and whichever answers is comma-split by load_backends — so
    // `backend = "headless,vnc"` is a list too.
    let mut backend_names: Vec<String> = Vec::new();
    if let Some(b) = &cli.backend {
        backend_names.extend(split_list(b));
    } else if !config.core.backends.is_empty() {
        backend_names.extend(config.core.backends.iter().cloned());
    } else if let Some(b) = &config.core.backend {
        backend_names.extend(split_list(b));
    } else {
        // C default: the native backend.
        backend_names.push("drm".to_string());
    }
    let mut backends = Vec::new();
    for name in &backend_names {
        let Some(b) = Backend::parse(name) else {
            // Wording kept from the C frontend: test_cli greps it.
            return Err(ConfigError::Invalid(format!("unknown backend \"{name}\"")));
        };
        if !backends.contains(&b) {
            backends.push(b);
        }
    }

    // C load_headless_backend/load_x11_backend: use-gl and use-pixman
    // are mutually exclusive, and neither may be combined with an
    // explicit --renderer.  Wording kept from the C frontend.
    if (cli.use_gl && cli.use_pixman) || (cli.renderer.is_some() && (cli.use_gl || cli.use_pixman))
    {
        return Err(ConfigError::Invalid(
            "Conflicting renderer specifications".to_string(),
        ));
    }
    let renderer_name = cli
        .renderer
        .clone()
        .or_else(|| {
            if cli.use_gl {
                Some("gl".into())
            } else if cli.use_pixman {
                Some("pixman".into())
            } else {
                None
            }
        })
        .or_else(|| config.core.renderer.clone());
    let renderer = match renderer_name.as_deref() {
        None | Some("auto") => Renderer::Auto,
        Some("gl") => Renderer::Gl,
        Some("pixman") => Renderer::Pixman,
        Some("noop") => Renderer::Noop,
        Some(other) => {
            return Err(ConfigError::Invalid(format!(
                "unknown renderer \"{other}\""
            )));
        }
    };

    let background_color = match &config.shell.background_color {
        Some(s) => parse_color(s, "[shell] background-color")?,
        None => 0xff002244,
    };

    let autolaunch = if !cli.autolaunch.is_empty() {
        Some(cli.autolaunch.clone())
    } else {
        config.autolaunch.path.clone().map(|p| vec![p])
    };

    let modules = match &cli.modules {
        Some(m) => split_list(m),
        None => config.core.modules.clone(),
    };

    // C parse_simple_mode only applies --width/--height when they are
    // non-zero, so a zero silently means "default" there and a negative
    // reaches the backend as a size.  Neither is useful: reject both up
    // front rather than hand a bad geometry to weston_windowed_output.
    for (name, value) in [
        ("--width", cli.width),
        ("--height", cli.height),
        ("--scale", cli.scale),
    ] {
        if let Some(v) = value
            && v <= 0
        {
            return Err(ConfigError::Invalid(format!(
                "{name} must be positive (got {v})"
            )));
        }
    }

    // Same rule for the per-output section scale.  C reads it with
    // weston_config_section_get_int and hands it straight to
    // weston_output_set_scale, whose `assert(output->current_scale)`
    // then aborts on 0 (and a negative scale produces nonsense
    // geometry) — reject both here, where --scale is already rejected.
    for out in &config.output {
        if let Some(v) = out.scale
            && v <= 0
        {
            let name = out.name.as_deref().unwrap_or("<unnamed>");
            return Err(ConfigError::Invalid(format!(
                "[output] '{name}': scale must be positive (got {v})"
            )));
        }
    }

    // Backend-specific sections belong to the backend that is actually
    // loaded: keying only off "vnc first, else rdp" would let a stray
    // [vnc] port hijack an RDP run.
    let has = |b: Backend| backends.contains(&b);
    let section_port = if has(Backend::Vnc) {
        config.vnc.port
    } else if has(Backend::Rdp) {
        config.rdp.port
    } else {
        None
    };

    Ok(Settings {
        backends,
        renderer,
        socket: cli.socket.clone(),
        log_file: cli.log.clone().map(PathBuf::from),
        debug_protocol: cli.debug,
        // C main.c:4585: the flag wins, and only when it is absent is
        // the config consulted -- so `[core] wait-for-debugger = false`
        // cannot cancel `--wait-for-debugger`.
        wait_for_debugger: cli.wait_for_debugger || config.core.wait_for_debugger,
        xwayland: cli.xwayland || config.core.xwayland,
        idle_time: cli.idle_time.or(config.core.idle_time),
        modules,
        require_input: config.core.require_input,
        color_management: config.core.color_management,
        gbm_format: config.core.gbm_format.clone(),
        background_color,
        shell_client: config.shell.client.clone(),
        width: cli.width,
        height: cli.height,
        scale: cli.scale,
        fullscreen: cli.fullscreen,
        output_count: cli.output_count,
        no_input: cli.no_input,
        sprawl: cli.sprawl,
        wayland_display: cli.display.clone(),
        cursor_theme: config.shell.cursor_theme.clone(),
        cursor_size: config.shell.cursor_size.unwrap_or(32),
        pipewire_num_outputs: config.pipewire.num_outputs,
        parent_display: cli.display.clone(),
        no_outputs: cli.no_outputs,
        transform: cli.transform.clone(),
        refresh_rate: cli.refresh_rate,
        drm_seat: cli.seat.clone(),
        drm_device: cli.drm_device.clone(),
        drm_additional_devices: cli.additional_devices.clone(),
        drm_current_mode: cli.current_mode,
        continue_without_input: cli.continue_without_input,
        rdp_vnc_port: cli.port.or(section_port),
        vnc_bind_address: cli.address.clone().or_else(|| config.vnc.address.clone()),
        vnc_refresh_rate: config.vnc.refresh_rate,
        vnc_disable_tls: cli.disable_transport_layer_security
            || config.vnc.disable_transport_layer_security.unwrap_or(false),
        rdp_tls_cert: cli
            .rdp_tls_cert
            .clone()
            .or_else(|| config.rdp.tls_cert.clone()),
        rdp_tls_key: cli
            .rdp_tls_key
            .clone()
            .or_else(|| config.rdp.tls_key.clone()),
        vnc_tls_cert: cli
            .vnc_tls_cert
            .clone()
            .or_else(|| config.vnc.tls_cert.clone()),
        vnc_tls_key: cli
            .vnc_tls_key
            .clone()
            .or_else(|| config.vnc.tls_key.clone()),
        rdp_external_listener_fd: cli.external_listener_fd.or(config.rdp.external_listener_fd),
        rdp_resizeable: !cli.no_resizeable && config.rdp.resizeable.unwrap_or(true),
        rdp_force_no_compression: cli.force_no_compression
            || config.rdp.force_no_compression.unwrap_or(false),
        rdp4_key: cli.rdp4_key.clone(),
        rdp_env_socket: cli.env_socket,
        rdp_remotefx_codec: !cli.no_remotefx_codec && config.rdp.remotefx_codec.unwrap_or(true),
        autolaunch,
        // C main.c execute_command: a positional command line is always
        // watched; config [autolaunch] watch applies otherwise.
        autolaunch_watch: config.autolaunch.watch || !cli.autolaunch.is_empty(),
        xwayland_path: config
            .xwayland
            .path
            .clone()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/usr/bin/Xwayland")),
        config_path,
        legacy_ini_found,
        config,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use clap::Parser;

    fn cli(args: &[&str]) -> Cli {
        Cli::parse_from(std::iter::once("westonite").chain(args.iter().copied()))
    }

    fn no_env() -> HashMap<String, String> {
        HashMap::new()
    }

    #[test]
    fn defaults_match_c() {
        let s = resolve_from(&cli(&["--no-config"]), &no_env()).unwrap();
        assert_eq!(s.backends, vec![Backend::Drm]);
        assert_eq!(s.renderer, Renderer::Auto);
        assert_eq!(s.background_color, 0xff002244);
        assert!(s.require_input);
        assert!(s.autolaunch.is_none());
        assert_eq!(s.xwayland_path, PathBuf::from("/usr/bin/Xwayland"));
    }

    #[test]
    fn unknown_backend_message_matches_c() {
        let err = resolve_from(&cli(&["--no-config", "--backend=bogus"]), &no_env())
            .unwrap_err()
            .to_string();
        assert_eq!(err, "unknown backend \"bogus\"");
    }

    #[test]
    fn file_then_override_then_flag_precedence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("westonite.toml");
        std::fs::write(
            &path,
            "[core]\nbackend = \"headless\"\nrenderer = \"pixman\"\n\
             [shell]\nbackground-color = \"0xff336699\"\n",
        )
        .unwrap();
        let c = cli(&[
            &format!("--config={}", path.display()),
            "-o",
            "core.renderer=noop",
            "--backend=vnc",
        ]);
        let s = resolve_from(&c, &no_env()).unwrap();
        assert_eq!(s.backends, vec![Backend::Vnc]); // flag beats file
        assert_eq!(s.renderer, Renderer::Noop); // -o beats file
        assert_eq!(s.background_color, 0xff336699);
    }

    #[test]
    fn unknown_key_is_a_startup_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("westonite.toml");
        std::fs::write(&path, "[core]\nbakend = \"headless\"\n").unwrap();
        let err = resolve_from(&cli(&[&format!("--config={}", path.display())]), &no_env())
            .unwrap_err()
            .to_string();
        assert!(err.contains("bakend"), "{err}");
    }

    #[test]
    fn xdg_discovery_and_legacy_ini_hint() {
        let dir = tempfile::tempdir().unwrap();
        let mut env = no_env();
        env.insert(
            "XDG_CONFIG_HOME".into(),
            dir.path().to_string_lossy().into_owned(),
        );
        // Only a legacy ini: ignored, but reported.
        std::fs::write(dir.path().join("westonite.ini"), "[core]\n").unwrap();
        let s = resolve_from(&cli(&[]), &env).unwrap();
        assert!(s.config_path.is_none());
        assert_eq!(s.legacy_ini_found, Some(dir.path().join("westonite.ini")));
        // A real toml wins.
        std::fs::write(dir.path().join("westonite.toml"), "[core]\n").unwrap();
        let s = resolve_from(&cli(&[]), &env).unwrap();
        assert_eq!(s.config_path, Some(dir.path().join("westonite.toml")));
    }

    #[test]
    fn home_fallback_when_no_xdg_config_home() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".config")).unwrap();
        std::fs::write(dir.path().join(".config/westonite.toml"), "[core]\n").unwrap();
        let mut env = no_env();
        env.insert("HOME".into(), dir.path().to_string_lossy().into_owned());
        let s = resolve_from(&cli(&[]), &env).unwrap();
        assert_eq!(
            s.config_path,
            Some(dir.path().join(".config/westonite.toml"))
        );
    }

    #[test]
    fn autolaunch_trailing_args_beat_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("westonite.toml");
        std::fs::write(
            &path,
            "[autolaunch]\npath = \"/bin/cfg-app\"\nwatch = true\n",
        )
        .unwrap();
        let c = cli(&[
            &format!("--config={}", path.display()),
            "--",
            "/bin/cli-app",
            "--flag",
        ]);
        let s = resolve_from(&c, &no_env()).unwrap();
        assert_eq!(
            s.autolaunch,
            Some(vec!["/bin/cli-app".to_string(), "--flag".to_string()])
        );
        assert!(s.autolaunch_watch);

        let s2 = resolve_from(&cli(&[&format!("--config={}", path.display())]), &no_env()).unwrap();
        assert_eq!(s2.autolaunch, Some(vec!["/bin/cfg-app".to_string()]));
    }

    #[test]
    fn positional_command_is_always_watched() {
        // C execute_command sets autolaunch_watch = true unconditionally.
        let s = resolve_from(&cli(&["--no-config", "--", "/bin/app"]), &no_env()).unwrap();
        assert!(s.autolaunch_watch);
        // ... while a config path without watch= stays unwatched.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("westonite.toml");
        std::fs::write(&path, "[autolaunch]\npath = \"/bin/cfg-app\"\n").unwrap();
        let s2 = resolve_from(&cli(&[&format!("--config={}", path.display())]), &no_env()).unwrap();
        assert!(!s2.autolaunch_watch);
    }

    #[test]
    fn hex_color_override_is_not_a_type_error() {
        // TOML parses 0x… as an integer, so the documented spelling
        // `-o shell.background-color=0xff336699` must still resolve.
        let s = resolve_from(
            &cli(&["--no-config", "-o", "shell.background-color=0xff336699"]),
            &no_env(),
        )
        .unwrap();
        assert_eq!(s.background_color, 0xff336699);
    }

    #[test]
    fn color_accepts_quoted_and_bare_spellings() {
        let dir = tempfile::tempdir().unwrap();
        for (i, body) in [
            "background-color = \"0xff336699\"",
            "background-color = 0xff336699",
            "background-color = 4281558681",
        ]
        .iter()
        .enumerate()
        {
            let path = dir.path().join(format!("c{i}.toml"));
            std::fs::write(&path, format!("[shell]\n{body}\n")).unwrap();
            let s =
                resolve_from(&cli(&[&format!("--config={}", path.display())]), &no_env()).unwrap();
            assert_eq!(s.background_color, 0xff336699, "{body}");
        }
    }

    #[test]
    fn list_keys_accept_comma_strings_and_arrays() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("westonite.toml");
        // The ini spelling survives the near-diagonal D11 mapping.
        std::fs::write(&path, "[core]\nbackends = \"headless,vnc\"\n").unwrap();
        let s = resolve_from(&cli(&[&format!("--config={}", path.display())]), &no_env()).unwrap();
        assert_eq!(s.backends, vec![Backend::Headless, Backend::Vnc]);

        std::fs::write(&path, "[core]\nmodules = [\"a.so\", \"b.so\"]\n").unwrap();
        let s = resolve_from(&cli(&[&format!("--config={}", path.display())]), &no_env()).unwrap();
        assert_eq!(s.modules, vec!["a.so".to_string(), "b.so".to_string()]);

        let s = resolve_from(
            &cli(&["--no-config", "-o", "core.modules=a.so,b.so"]),
            &no_env(),
        )
        .unwrap();
        assert_eq!(s.modules, vec!["a.so".to_string(), "b.so".to_string()]);
    }

    #[test]
    fn backend_and_backends_are_one_variable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("westonite.toml");
        let conf = |args: &[&str]| {
            let mut argv = vec![format!("--config={}", path.display())];
            argv.extend(args.iter().map(|a| a.to_string()));
            let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
            resolve_from(&cli(&argv), &no_env()).unwrap().backends
        };

        // C main.c:4602-4606 reads `backends` first and falls back to
        // `backend`; with both present, `backends` wins.
        std::fs::write(
            &path,
            "[core]\nbackend = \"headless\"\nbackends = \"vnc,headless\"\n",
        )
        .unwrap();
        assert_eq!(conf(&[]), vec![Backend::Vnc, Backend::Headless]);

        // The singular key feeds the same comma-split variable.
        std::fs::write(&path, "[core]\nbackend = \"headless,vnc\"\n").unwrap();
        assert_eq!(conf(&[]), vec![Backend::Headless, Backend::Vnc]);

        // Either CLI spelling takes a list, and the last one wins.
        assert_eq!(
            conf(&["--backend=vnc", "--backends=headless,vnc"]),
            vec![Backend::Headless, Backend::Vnc]
        );
        assert_eq!(
            conf(&["--backends=headless,vnc", "--backend=vnc"]),
            vec![Backend::Vnc]
        );
    }

    #[test]
    fn xdg_config_home_falls_through_to_home_config() {
        // libweston's open_config_file tries $XDG_CONFIG_HOME and then
        // $HOME/.config; an empty XDG_CONFIG_HOME must not mask a config
        // sitting in the home fallback.
        let dir = tempfile::tempdir().unwrap();
        let xdg = dir.path().join("xdg");
        let home = dir.path().join("home");
        std::fs::create_dir_all(&xdg).unwrap();
        std::fs::create_dir_all(home.join(".config")).unwrap();
        std::fs::write(home.join(".config/westonite.toml"), "[core]\n").unwrap();
        let mut env = no_env();
        env.insert("XDG_CONFIG_HOME".into(), xdg.to_string_lossy().into_owned());
        env.insert("HOME".into(), home.to_string_lossy().into_owned());
        let s = resolve_from(&cli(&[]), &env).unwrap();
        assert_eq!(s.config_path, Some(home.join(".config/westonite.toml")));
    }

    #[test]
    fn conflicting_renderer_flags_are_rejected() {
        for args in [
            vec!["--no-config", "--use-gl", "--use-pixman"],
            vec!["--no-config", "--renderer=gl", "--use-pixman"],
        ] {
            let err = resolve_from(&cli(&args), &no_env())
                .unwrap_err()
                .to_string();
            assert_eq!(err, "Conflicting renderer specifications", "{args:?}");
        }
    }

    #[test]
    fn non_positive_geometry_is_rejected() {
        for (flag, args) in [
            ("--width", vec!["--no-config", "--width=0"]),
            ("--height", vec!["--no-config", "--height=-1"]),
            ("--scale", vec!["--no-config", "--scale=0"]),
        ] {
            let err = resolve_from(&cli(&args), &no_env())
                .unwrap_err()
                .to_string();
            assert!(err.starts_with(flag), "{err}");
        }
    }

    #[test]
    fn non_positive_output_section_scale_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("westonite.toml");
        std::fs::write(&path, "[[output]]\nname = \"headless\"\nscale = 0\n").unwrap();
        let cfg = format!("--config={}", path.display());
        let err = resolve_from(&cli(&[&cfg]), &no_env())
            .unwrap_err()
            .to_string();
        assert_eq!(err, "[output] 'headless': scale must be positive (got 0)");

        std::fs::write(&path, "[[output]]\nname = \"headless\"\nscale = 2\n").unwrap();
        let s = resolve_from(&cli(&[&cfg]), &no_env()).unwrap();
        assert_eq!(s.config.output[0].scale, Some(2));
    }

    #[test]
    fn backend_sections_do_not_leak_across_backends() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("westonite.toml");
        std::fs::write(&path, "[vnc]\nport = 5900\n[rdp]\nport = 3389\n").unwrap();
        let cfg = format!("--config={}", path.display());
        let s = resolve_from(&cli(&[&cfg, "--backend=rdp"]), &no_env()).unwrap();
        assert_eq!(s.rdp_vnc_port, Some(3389));
        let s = resolve_from(&cli(&[&cfg, "--backend=vnc"]), &no_env()).unwrap();
        assert_eq!(s.rdp_vnc_port, Some(5900));
        // Neither backend loaded: no port at all, rather than a stray one.
        let s = resolve_from(&cli(&[&cfg, "--backend=headless"]), &no_env()).unwrap();
        assert_eq!(s.rdp_vnc_port, None);
        // --port still wins.
        let s = resolve_from(&cli(&[&cfg, "--backend=vnc", "--port=1234"]), &no_env()).unwrap();
        assert_eq!(s.rdp_vnc_port, Some(1234));
    }

    #[test]
    fn output_sections_pass_through_typed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("westonite.toml");
        std::fs::write(
            &path,
            "[[output]]\nname = \"headless\"\nmode = \"800x600\"\nscale = 2\n\
             [[output]]\nname = \"X1\"\nmode = \"off\"\n",
        )
        .unwrap();
        let s = resolve_from(&cli(&[&format!("--config={}", path.display())]), &no_env()).unwrap();
        assert_eq!(s.config.output.len(), 2);
        assert_eq!(s.config.output[0].scale, Some(2));
        assert_eq!(s.config.output[1].mode.as_deref(), Some("off"));
    }
}
