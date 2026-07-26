//! Resolution (D9–D12): defaults → file → `-o` overrides → flags, into
//! an immutable [`Settings`] resolved once at startup.
//!
//! File discovery mirrors the ini search the C frontend had (P2), with
//! the new name: `$XDG_CONFIG_HOME/westonite.toml`, then
//! `$HOME/.config/westonite.toml`, then each of `$XDG_CONFIG_DIRS`
//! (default `/etc/xdg`).  `WESTON_CONFIG_FILE` is dropped (D12).  If a
//! legacy `westonite.ini` sits where the TOML is expected, resolution
//! reports a one-line hint and otherwise ignores it (D11).

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
    pub logger_scopes: Vec<String>,
    pub flight_rec_scopes: Vec<String>,
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
    pub fullscreen: bool,
    pub output_count: Option<u32>,
    pub no_input: bool,
    pub sprawl: bool,
    pub parent_display: Option<String>,
    pub no_outputs: bool,
    pub refresh_rate: Option<i32>,

    pub drm_seat: Option<String>,
    pub drm_device: Option<String>,
    pub drm_additional_devices: Option<String>,
    pub drm_current_mode: bool,
    pub continue_without_input: bool,

    pub rdp_vnc_port: Option<u16>,
    pub vnc_disable_tls: bool,

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
    if let Some(x) = env.get("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        dirs.push(PathBuf::from(x));
    } else if let Some(h) = env.get("HOME").filter(|v| !v.is_empty()) {
        dirs.push(Path::new(h).join(".config"));
    }
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
    let mut backend_names: Vec<String> = Vec::new();
    if let Some(b) = &cli.backend {
        backend_names.push(b.clone());
    } else if let Some(bs) = &cli.backends {
        backend_names.extend(split_list(bs));
    } else if let Some(b) = &config.core.backend {
        backend_names.push(b.clone());
    } else if !config.core.backends.is_empty() {
        backend_names.extend(config.core.backends.iter().cloned());
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

    Ok(Settings {
        backends,
        renderer,
        socket: cli.socket.clone(),
        log_file: cli.log.clone().map(PathBuf::from),
        debug_protocol: cli.debug,
        logger_scopes: cli
            .logger_scopes
            .as_deref()
            .map(split_list)
            .unwrap_or_default(),
        flight_rec_scopes: cli
            .flight_rec_scopes
            .as_deref()
            .map(split_list)
            .unwrap_or_default(),
        wait_for_debugger: cli.wait_for_debugger,
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
        parent_display: cli.display.clone(),
        no_outputs: cli.no_outputs,
        refresh_rate: cli.refresh_rate,
        drm_seat: cli.seat.clone(),
        drm_device: cli.drm_device.clone(),
        drm_additional_devices: cli.additional_devices.clone(),
        drm_current_mode: cli.current_mode,
        continue_without_input: cli.continue_without_input,
        rdp_vnc_port: cli.port.or(config.vnc.port).or(config.rdp.port),
        vnc_disable_tls: cli.disable_transport_layer_security
            || config.vnc.disable_transport_layer_security.unwrap_or(false),
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
