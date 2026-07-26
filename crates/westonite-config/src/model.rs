//! The serde `Config` model: the entire `westonite.toml` surface.
//!
//! Kebab-case keys throughout (one `rename_all`), `deny_unknown_fields`
//! everywhere: a typo'd key is a startup error with a span, replacing
//! weston's silent-typo behavior (D9).  Defaults here mirror the C
//! frontend's defaults exactly; deviations are §10 material.

use serde::Deserialize;

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct Config {
    pub core: Core,
    pub shell: Shell,
    pub keyboard: Keyboard,
    pub libinput: Libinput,
    pub autolaunch: Autolaunch,
    pub xwayland: Xwayland,
    pub rdp: Rdp,
    pub vnc: Vnc,
    pub pipewire: Pipewire,
    /// Repeated `[output]` ini sections become `[[output]]`.
    pub output: Vec<Output>,
    /// `[remote-output]` gstreamer streams (remoting plugin).
    pub remote_output: Vec<RemoteOutput>,
    /// `[pipewire-output]` streams.
    pub pipewire_output: Vec<PipewireOutput>,
    /// `[color_characteristics]` ini sections, referenced from outputs
    /// by name.
    pub color_characteristics: Vec<ColorCharacteristics>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct Core {
    /// `[core] backend=` / `--backend`.  C default: drm
    /// (WESTON_NATIVE_BACKEND); resolution keeps that default.
    pub backend: Option<String>,
    /// `[core] backends=` / `--backends` (multi-backend, weston 14).
    pub backends: Vec<String>,
    /// `[core] renderer=` / `--renderer`: auto|gl|pixman|noop.
    pub renderer: Option<String>,
    /// `[core] gbm-format=`.
    pub gbm_format: Option<String>,
    /// `[core] require-input=`.  C default true.
    #[serde(default = "default_true")]
    pub require_input: bool,
    /// `[core] color-management=`.
    pub color_management: bool,
    /// `[core] xwayland=` / `--xwayland`.
    pub xwayland: bool,
    /// `[core] idle-time=` (inert since T3 — kept for surface
    /// completeness, D1).
    pub idle_time: Option<u32>,
    /// `[core] modules=` / `--modules`: extra wet_module_init plugins.
    pub modules: Vec<String>,
    /// `[core] shell=` / `--shell`.  C default desktop-shell.so; the
    /// Rust frontend links its shell statically at R3 (D2) and treats
    /// any non-default value as a startup error then.
    pub shell: Option<String>,
}

impl Default for Core {
    fn default() -> Self {
        Core {
            backend: None,
            backends: Vec::new(),
            renderer: None,
            gbm_format: None,
            require_input: true,
            color_management: false,
            xwayland: false,
            idle_time: None,
            modules: Vec::new(),
            shell: None,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct Shell {
    /// `[shell] background-color=`, 0xAARRGGBB (string to keep the C
    /// hex spelling; parsed at resolve time).  C default 0xff002244.
    pub background_color: Option<String>,
    /// `[shell] client=` — empty means "no helper client" (P3); kept
    /// for surface completeness.
    pub client: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct Keyboard {
    pub keymap_rules: Option<String>,
    pub keymap_model: Option<String>,
    pub keymap_layout: Option<String>,
    pub keymap_variant: Option<String>,
    pub keymap_options: Option<String>,
    pub repeat_rate: Option<u32>,
    pub repeat_delay: Option<u32>,
    pub numlock_on: Option<bool>,
    pub vt_switching: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct Libinput {
    pub enable_tap: Option<bool>,
    pub tap_and_drag: Option<bool>,
    pub tap_and_drag_lock: Option<bool>,
    pub natural_scroll: Option<bool>,
    pub left_handed: Option<bool>,
    pub middle_button_emulation: Option<bool>,
    pub rotation: Option<u32>,
    pub accel_profile: Option<String>,
    pub accel_speed: Option<f64>,
    pub scroll_method: Option<String>,
    pub scroll_button: Option<String>,
    pub touchscreen_calibrator: Option<bool>,
    pub calibration_helper: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct Autolaunch {
    /// `[autolaunch] path=`: client spawned at startup.
    pub path: Option<String>,
    /// `[autolaunch] watch=`: exit the compositor when it exits.
    pub watch: bool,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct Xwayland {
    /// `[xwayland] path=`.  C default /usr/bin/Xwayland.
    pub path: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct Rdp {
    pub port: Option<u16>,
    pub address: Option<String>,
    pub no_clients_resize: Option<bool>,
    pub force_no_compression: Option<bool>,
    pub remotefx_codec: Option<bool>,
    pub external_listener_fd: Option<i32>,
    pub refresh_rate: Option<u32>,
    pub tls_cert: Option<String>,
    pub tls_key: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct Vnc {
    pub port: Option<u16>,
    pub refresh_rate: Option<u32>,
    pub tls_cert: Option<String>,
    pub tls_key: Option<String>,
    pub disable_transport_layer_security: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct Pipewire {
    pub num_outputs: Option<u32>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct Output {
    /// Head name this section configures (C `[output] name=`).
    pub name: Option<String>,
    /// Modeline / preferred / current / off (weston grammar, kept as a
    /// string — §5 "don't over-model").
    pub mode: Option<String>,
    pub scale: Option<i32>,
    pub transform: Option<String>,
    /// Explicit layout position "x,y" (weston grammar).
    pub position: Option<String>,
    /// Same-CRTC clone (DRM).
    pub clone_of: Option<String>,
    /// Mirror onto another head (P0 territory).
    pub mirror_of: Option<String>,
    /// DRM/windowed extras, all weston grammars:
    pub seat: Option<String>,
    pub gbm_format: Option<String>,
    pub pixman_shadow: Option<bool>,
    pub icc_profile: Option<String>,
    pub eotf_mode: Option<String>,
    pub colorimetry_mode: Option<String>,
    /// Name of a `[[color-characteristics]]` block.
    pub color_characteristics: Option<String>,
    pub max_bpc: Option<u32>,
    pub vrr_mode: Option<String>,
    /// "true" disables the output (C `mode=off` alternative surface).
    pub off: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct RemoteOutput {
    pub name: Option<String>,
    pub mode: Option<String>,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub gst_pipeline: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct PipewireOutput {
    pub name: Option<String>,
    pub mode: Option<String>,
    pub gbm_format: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct ColorCharacteristics {
    /// Referenced from `[[output]] color-characteristics = name`.
    pub name: Option<String>,
    pub maximum_luminance: Option<f64>,
    pub minimum_luminance: Option<f64>,
    pub max_cll: Option<f64>,
    pub max_fall: Option<f64>,
    pub red_x: Option<f64>,
    pub red_y: Option<f64>,
    pub green_x: Option<f64>,
    pub green_y: Option<f64>,
    pub blue_x: Option<f64>,
    pub blue_y: Option<f64>,
    pub white_x: Option<f64>,
    pub white_y: Option<f64>,
}
