//! The clap CLI (D10): ergonomic flags for today's CLI surface, plus
//! the generic `-o key.path=value` dotted override that patches the
//! config tree before deserialization — 100% CLI coverage of the file
//! surface without bespoke flags for structured sections.
//!
//! Flag spellings keep the C frontend's names (docs R-G checklist);
//! e2e test_cli drives them, so `--backend=headless`, `--log=`,
//! `--socket=`, `--width/height`, `--no-config`, `--config` behave as
//! before.  Trailing positional args remain the autolaunch command.

use clap::Parser;

#[derive(Debug, Clone, Parser, Default)]
#[command(
    name = "westonite",
    disable_version_flag = true,
    about = "Standalone Weston-based Wayland compositor",
    after_help = "Any [section] key of westonite.toml can be set with \
                  -o section.key=value; trailing arguments are launched \
                  as the autolaunch client (put -- before them when they \
                  carry flags of their own)."
)]
pub struct Cli {
    /// Print version and exit.
    #[arg(long)]
    pub version: bool,

    /// Backend(s) to load, comma-separated (headless, drm, x11,
    /// wayland, rdp, vnc, pipewire).  `--backends` is the same flag
    /// under its C spelling — in C both options write one variable, so
    /// the last occurrence wins and either accepts a list.
    /// (Self-override: C's option table lets the flag repeat with the
    /// last occurrence winning; clap needs that stated explicitly.)
    #[arg(
        long,
        short = 'B',
        visible_alias = "backends",
        overrides_with = "backend"
    )]
    pub backend: Option<String>,
    /// Renderer: auto, gl, pixman, noop.
    #[arg(long)]
    pub renderer: Option<String>,
    /// Legacy renderer toggles (per-backend in C; kept as aliases).
    #[arg(long)]
    pub use_gl: bool,
    #[arg(long)]
    pub use_pixman: bool,

    /// Wayland socket name to bind (default: automatic).
    #[arg(long, short = 'S')]
    pub socket: Option<String>,
    /// Log file path.
    #[arg(long)]
    pub log: Option<String>,
    /// Config file path (default: XDG search for westonite.toml).
    #[arg(long, short = 'c')]
    pub config: Option<String>,
    /// Do not read any config file.
    #[arg(long)]
    pub no_config: bool,
    /// Generic override: -o section.key=value (repeatable; applied to
    /// the config tree after the file, before flags).
    #[arg(short = 'o', long = "set", value_name = "KEY.PATH=VALUE")]
    pub set: Vec<String>,

    /// Wait for a debugger before continuing.
    #[arg(long)]
    pub wait_for_debugger: bool,
    /// Enable the weston-debug protocol.
    #[arg(long)]
    pub debug: bool,
    /// Log scopes to subscribe the logger to (comma-separated).
    #[arg(long)]
    pub logger_scopes: Option<String>,
    /// Log scopes for the in-memory flight recorder.
    #[arg(long)]
    pub flight_rec_scopes: Option<String>,

    // -- windowed/headless backend options --
    /// Output width (headless, x11, wayland, vnc, rdp, pipewire).
    #[arg(long)]
    pub width: Option<i32>,
    /// Output height.
    #[arg(long)]
    pub height: Option<i32>,
    /// Output scale (x11/wayland).
    #[arg(long)]
    pub scale: Option<i32>,
    /// Output transform: normal, rotate-90/180/270,
    /// flipped[-rotate-90/180/270] (headless, x11, wayland).
    #[arg(long)]
    pub transform: Option<String>,
    /// Fullscreen (x11/wayland nested).
    #[arg(long)]
    pub fullscreen: bool,
    /// Number of outputs (x11).
    #[arg(long)]
    pub output_count: Option<u32>,
    /// Disable input devices (x11).
    #[arg(long)]
    pub no_input: bool,
    /// One output per parent output (wayland nested).
    #[arg(long)]
    pub sprawl: bool,
    /// Parent wayland display (wayland nested).
    #[arg(long)]
    pub display: Option<String>,
    /// Create no outputs (headless).
    #[arg(long)]
    pub no_outputs: bool,
    /// Output repaint rate in mHz (headless only — C gives rdp/vnc no
    /// such flag; their Hz rate comes from `[rdp]`/`[vnc] refresh-rate`).
    #[arg(long)]
    pub refresh_rate: Option<i32>,

    // -- drm --
    /// libinput seat id (drm).
    #[arg(long)]
    pub seat: Option<String>,
    /// Primary DRM device.
    #[arg(long)]
    pub drm_device: Option<String>,
    /// Secondary DRM devices (comma-separated).
    #[arg(long)]
    pub additional_devices: Option<String>,
    /// Reuse the current CRTC mode.
    #[arg(long)]
    pub current_mode: bool,
    /// Keep running when no input device is present.
    #[arg(long)]
    pub continue_without_input: bool,

    // -- rdp/vnc --
    /// Listen port (rdp/vnc).
    #[arg(long)]
    pub port: Option<u16>,
    /// Bind address for the listener (vnc; default: all interfaces).
    #[arg(long)]
    pub address: Option<String>,
    /// TLS certificate (rdp/vnc).
    #[arg(long)]
    pub rdp_tls_cert: Option<String>,
    #[arg(long)]
    pub rdp_tls_key: Option<String>,
    #[arg(long)]
    pub vnc_tls_cert: Option<String>,
    #[arg(long)]
    pub vnc_tls_key: Option<String>,
    /// Disable TLS entirely (vnc).
    #[arg(long)]
    pub disable_transport_layer_security: bool,
    /// External listener fd (rdp).
    #[arg(long)]
    pub external_listener_fd: Option<i32>,
    /// No client-driven resize (rdp; C --no-resizeable).
    #[arg(long)]
    pub no_resizeable: bool,
    /// RDP4-style pre-shared key file (rdp; C --rdp4-key).
    #[arg(long)]
    pub rdp4_key: Option<String>,
    /// Use an inherited socket instead of listening (rdp; C
    /// --env-socket).
    #[arg(long)]
    pub env_socket: bool,
    /// Disable the RemoteFX codec (rdp; C --no-remotefx-codec).
    #[arg(long)]
    pub no_remotefx_codec: bool,
    /// Disable compression (rdp).
    #[arg(long)]
    pub force_no_compression: bool,

    // -- misc parity flags --
    /// Load Xwayland support.
    #[arg(long)]
    pub xwayland: bool,
    /// Idle timeout in seconds (inert since T3; accepted for parity).
    #[arg(long)]
    pub idle_time: Option<u32>,
    /// Extra wet_module_init plugins (comma-separated).
    #[arg(long)]
    pub modules: Option<String>,
    /// Shell plugin (parity flag; the Rust frontend's shell is built in
    /// and only the default value is accepted at R3).
    #[arg(long)]
    pub shell: Option<String>,

    /// Autolaunch client command (trailing args; use `--` before flags
    /// belonging to the client).
    #[arg(trailing_var_arg = true, allow_hyphen_values = false)]
    pub autolaunch: Vec<String>,
}
