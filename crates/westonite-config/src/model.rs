//! The serde `Config` model: the entire `westonite.toml` surface.
//!
//! Kebab-case keys throughout (one `rename_all`), `deny_unknown_fields`
//! everywhere: a typo'd key is a startup error with a span, replacing
//! weston's silent-typo behavior (D9).  Defaults here mirror the C
//! frontend's defaults exactly; deviations are §10 material.

use std::fmt;

use serde::Deserialize;
use serde::de;

fn default_true() -> bool {
    true
}

/// Accept a quoted string *or* a bare number for keys whose value is
/// really a numeric string in weston's grammars (`[shell]
/// background-color`, `[libinput] scroll-button`).
///
/// Needed because `-o` overrides are parsed as TOML values before
/// deserialization (`overrides.rs`), and TOML reads `0xff002244` as an
/// integer — so the very spelling the docs advertise,
/// `-o shell.background-color=0xff002244`, would otherwise die as
/// "invalid type: integer, expected a string".  Numbers are handed on
/// in decimal; `resolve::parse_color` accepts decimal as well as the
/// `0x` spelling.
fn de_opt_scalar_string<'de, D>(d: D) -> Result<Option<String>, D::Error>
where
    D: de::Deserializer<'de>,
{
    struct V;
    impl<'de> de::Visitor<'de> for V {
        type Value = Option<String>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a string or a number")
        }
        fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
            Ok(Some(v.to_string()))
        }
        fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
            Ok(Some(v.to_string()))
        }
        fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
            Ok(Some(v.to_string()))
        }
        fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }
        fn visit_some<D2: de::Deserializer<'de>>(self, d: D2) -> Result<Self::Value, D2::Error> {
            d.deserialize_any(V)
        }
    }
    d.deserialize_any(V)
}

/// Accept a TOML array *or* one comma-separated string for list keys
/// (`[core] backends`, `[core] modules`).  The array is the documented
/// TOML spelling; the comma list keeps the ini spelling working, which
/// the `--backends`/`--modules` flags and `-o core.backends=drm,vnc`
/// both produce (an override value never arrives pre-typed as an
/// array unless the user writes TOML array syntax by hand).
fn de_string_or_seq<'de, D>(d: D) -> Result<Vec<String>, D::Error>
where
    D: de::Deserializer<'de>,
{
    struct V;
    impl<'de> de::Visitor<'de> for V {
        type Value = Vec<String>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("an array of strings or a comma-separated string")
        }
        fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
            Ok(v.split(',')
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .map(String::from)
                .collect())
        }
        fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut out = Vec::new();
            while let Some(item) = seq.next_element::<String>()? {
                out.push(item);
            }
            Ok(out)
        }
    }
    d.deserialize_any(V)
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
    /// Array, or one comma-separated string (the ini spelling).
    #[serde(default, deserialize_with = "de_string_or_seq")]
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
    /// `[core] pageflip-timeout=` (ms, DRM only; 0 disables).
    pub pageflip_timeout: Option<u32>,
    /// `[core] pixman-shadow=` (DRM only).  C default true.
    pub pixman_shadow: Option<bool>,
    /// `[core] require-outputs=`: any|all|none.  C default "any" —
    /// DRM only, since it is the only backend whose outputs can fail
    /// to come up (main.c:4644).
    pub require_outputs: Option<String>,
    /// `[core] wait-for-debugger=`: log the pid and SIGSTOP at
    /// startup.  The CLI flag wins; this only applies when it is
    /// absent (main.c:4585).
    pub wait_for_debugger: bool,
    /// `[core] repaint-window=` (ms; C validates -10..=1000).
    pub repaint_window: Option<i32>,
    /// `[core] modules=` / `--modules`: extra wet_module_init plugins.
    /// Array, or one comma-separated string (the ini spelling).
    #[serde(default, deserialize_with = "de_string_or_seq")]
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
            pageflip_timeout: None,
            pixman_shadow: None,
            require_outputs: None,
            wait_for_debugger: false,
            repaint_window: None,
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
    /// A bare TOML number is accepted too — see `de_opt_scalar_string`.
    #[serde(default, deserialize_with = "de_opt_scalar_string")]
    pub background_color: Option<String>,
    /// `[shell] client=` — empty means "no helper client" (P3); kept
    /// for surface completeness.
    pub client: Option<String>,
    /// `[shell] cursor-theme=` / `cursor-size=`: read by the nested
    /// wayland backend for the cursor it draws on the parent
    /// compositor (C load_wayland_backend, main.c:4084).  C default
    /// size 32.
    pub cursor_theme: Option<String>,
    pub cursor_size: Option<i32>,
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
    /// `[libinput] disable-while-typing=` (touchpads).
    pub disable_while_typing: Option<bool>,
    pub natural_scroll: Option<bool>,
    pub left_handed: Option<bool>,
    pub middle_button_emulation: Option<bool>,
    pub rotation: Option<u32>,
    pub accel_profile: Option<String>,
    pub accel_speed: Option<f64>,
    pub scroll_method: Option<String>,
    /// evdev button *name*, e.g. `BTN_RIGHT` — that is all
    /// `libevdev_event_code_from_name` accepts, in C too.  The scalar
    /// deserializer is kept so a numeric spelling reaches the frontend
    /// as a string and is rejected with a message rather than a serde
    /// type error.
    #[serde(default, deserialize_with = "de_opt_scalar_string")]
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
    /// C config field `resizeable` (CLI spelling --no-resizeable).
    pub resizeable: Option<bool>,
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
    /// Hz (C VNC_DEFAULT_FREQ 60).
    pub refresh_rate: Option<u32>,
    /// Bind address (C --address only; the section spelling is a
    /// re-spec addition for CLI/file symmetry).
    pub address: Option<String>,
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
    /// `[output] content-type=` (DRM): the HDMI content-type hint.
    pub content_type: Option<String>,
    /// `[output] force-on=` (DRM): enable the head even when the
    /// connector reads disconnected (C drm_head_should_force_enable).
    pub force_on: Option<bool>,
    /// `[output] resizeable=` (vnc/rdp: client-driven desktop resize;
    /// C default true).
    pub resizeable: Option<bool>,
    /// "true" disables the output (C `mode=off` alternative surface).
    pub off: Option<bool>,
    /// `[output] allow-hdcp=` (C's `allow_hdcp`), default true.  Read
    /// by every backend's configure, not just DRM.
    pub allow_hdcp: Option<bool>,
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
    /// C's ini spellings are `max_L` / `min_L` / `maxFALL`; the TOML
    /// model is kebab-case throughout (D11).
    pub max_luminance: Option<f64>,
    pub min_luminance: Option<f64>,
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
