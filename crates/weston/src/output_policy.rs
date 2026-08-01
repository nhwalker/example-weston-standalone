//! Output configuration policy (plan §7 R2b).
//!
//! Plain data, not callbacks: the frontend resolves its `Settings` into
//! an [`OutputPolicy`] once at startup and hands it to the builder.
//! The sync-tier heads-changed handler (§3e) consults it per head —
//! since deciding is a pure table lookup, the A3 proof for that
//! trampoline stays "wrapper state only, no app borrow".
//!
//! Precedence per head mirrors C `wet_configure_windowed_output_from_
//! config`: backend defaults → the name-matched `[[output]]` section →
//! CLI overrides.

/// wl_output_transform, typed (C main.c `transforms[]` grammar).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputTransform {
    #[default]
    Normal,
    Rotate90,
    Rotate180,
    Rotate270,
    Flipped,
    FlippedRotate90,
    FlippedRotate180,
    FlippedRotate270,
}

impl OutputTransform {
    /// C `weston_parse_transform` names, exactly.
    pub fn parse(s: &str) -> Option<OutputTransform> {
        Some(match s {
            "normal" => OutputTransform::Normal,
            "rotate-90" => OutputTransform::Rotate90,
            "rotate-180" => OutputTransform::Rotate180,
            "rotate-270" => OutputTransform::Rotate270,
            "flipped" => OutputTransform::Flipped,
            "flipped-rotate-90" => OutputTransform::FlippedRotate90,
            "flipped-rotate-180" => OutputTransform::FlippedRotate180,
            "flipped-rotate-270" => OutputTransform::FlippedRotate270,
            _ => return None,
        })
    }

    pub(crate) fn to_c(self) -> weston_sys::wl_output_transform::Type {
        use weston_sys::wl_output_transform as t;
        match self {
            OutputTransform::Normal => t::WL_OUTPUT_TRANSFORM_NORMAL,
            OutputTransform::Rotate90 => t::WL_OUTPUT_TRANSFORM_90,
            OutputTransform::Rotate180 => t::WL_OUTPUT_TRANSFORM_180,
            OutputTransform::Rotate270 => t::WL_OUTPUT_TRANSFORM_270,
            OutputTransform::Flipped => t::WL_OUTPUT_TRANSFORM_FLIPPED,
            OutputTransform::FlippedRotate90 => t::WL_OUTPUT_TRANSFORM_FLIPPED_90,
            OutputTransform::FlippedRotate180 => t::WL_OUTPUT_TRANSFORM_FLIPPED_180,
            OutputTransform::FlippedRotate270 => t::WL_OUTPUT_TRANSFORM_FLIPPED_270,
        }
    }
}

/// One `[[output]]` section, already parsed by the frontend (mode
/// string → size, transform string → enum; parse errors were startup
/// errors there).
#[derive(Debug, Clone, Default)]
pub struct OutputRule {
    /// Head name this rule configures (C `[output] name=`).
    pub name: String,
    /// `off = true` / `mode = "off"`: leave this head unenabled.
    pub off: bool,
    /// Parsed `mode = "WxH"`, if present and valid.
    pub size: Option<(i32, i32)>,
    pub scale: Option<i32>,
    pub transform: Option<OutputTransform>,
    /// `[output] resizeable=` (vnc/rdp only; C default true).
    pub resizeable: Option<bool>,
    /// `[output] gbm-format=` — read per output by both the pipewire
    /// and the DRM configure.
    pub gbm_format: Option<String>,
    /// The raw `mode =` string, kept alongside the parsed `size` for
    /// the DRM path: there, "preferred" / "current" / a modeline are
    /// all meaningful and only the backend can resolve a modeline, so
    /// the frontend must not reduce it to a WxH pair.
    pub mode_string: Option<String>,
    /// `[output] max-bpc=` (DRM only, C default 16 — see
    /// [`OutputPolicy::decide_drm`] for the mode=current interaction).
    pub max_bpc: Option<u32>,
    /// `[output] content-type=` (DRM only).
    pub content_type: Option<String>,
    /// `[output] seat=` (DRM only; C default "").
    pub seat: Option<String>,
    /// `[output] force-on=` (DRM only): enable the head even when the
    /// connector reads disconnected.
    pub force_on: bool,
    /// `[output] clone-of=` (DRM only): this section contributes
    /// nothing itself — the named section controls the head instead.
    pub clone_of: Option<String>,
    /// The colour slice: icc-profile / eotf-mode / colorimetry-mode /
    /// color-characteristics / allow-hdcp.  Already validated by the
    /// frontend (mode names, characteristic groups).
    pub color: ColorSetup,
    /// `[output] mirror-of=` — the *source* output this (remote) head
    /// mirrors (C wet_config_find_output_mirror family).  The rule's
    /// `name` is the remote head; `mirror_of` names the native output
    /// whose content it clones.
    pub mirror_of: Option<String>,
}

/// CLI-level overrides (win over any section, C parsed_options).
#[derive(Debug, Clone, Copy, Default)]
pub struct OutputCliOverrides {
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub scale: Option<i32>,
    pub transform: Option<OutputTransform>,
}

/// The resolved per-head decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputSetup {
    pub width: i32,
    pub height: i32,
    pub scale: i32,
    pub transform: OutputTransform,
}

/// The whole policy: defaults come from the backend (headless:
/// 1024x640, scale 1, normal — C headless_backend_output_configure).
#[derive(Debug, Clone)]
pub struct OutputPolicy {
    pub default_size: (i32, i32),
    pub default_scale: i32,
    pub default_transform: OutputTransform,
    pub cli: OutputCliOverrides,
    pub rules: Vec<OutputRule>,
}

impl OutputPolicy {
    /// Backend defaults only (the R0 smoke path).
    pub fn defaults(width: i32, height: i32) -> OutputPolicy {
        OutputPolicy {
            default_size: (width, height),
            default_scale: 1,
            default_transform: OutputTransform::Normal,
            cli: OutputCliOverrides::default(),
            rules: Vec::new(),
        }
    }

    /// Decide the setup for a head.  `None` = leave the head unenabled
    /// (`off`).  Precedence: defaults → section → CLI (C
    /// parse_simple_mode / wet_output_set_scale / _set_transform).
    pub fn decide(&self, head_name: &str) -> Option<OutputSetup> {
        self.decide_sized(head_name, self.default_size)
    }

    /// As [`OutputPolicy::decide`], but with the backend's own default
    /// size.  Every windowed backend shares
    /// `wet_configure_windowed_output_from_config` and differs only in
    /// the `wet_output_config defaults` it passes: headless 1024x640
    /// (main.c:3438), x11 1024x600 (3910), wayland 1024x640 (4030).
    pub fn decide_sized(&self, head_name: &str, default_size: (i32, i32)) -> Option<OutputSetup> {
        let rule = self.rules.iter().find(|r| r.name == head_name);
        if let Some(r) = rule
            && r.off
        {
            return None;
        }
        let (mut width, mut height) = default_size;
        let mut scale = self.default_scale;
        let mut transform = self.default_transform;
        if let Some(r) = rule {
            if let Some((w, h)) = r.size {
                width = w;
                height = h;
            }
            if let Some(s) = r.scale {
                scale = s;
            }
            if let Some(t) = r.transform {
                transform = t;
            }
        }
        if let Some(w) = self.cli.width {
            width = w;
        }
        if let Some(h) = self.cli.height {
            height = h;
        }
        if let Some(s) = self.cli.scale {
            scale = s;
        }
        if let Some(t) = self.cli.transform {
            transform = t;
        }
        Some(OutputSetup {
            width,
            height,
            scale,
            transform,
        })
    }

    /// Decide the setup for a VNC head — C `vnc_backend_output_configure`
    /// applies a narrower slice of the surface than the windowed path:
    /// defaults are 640x480; `mode=` and CLI `--width/--height` set the
    /// size (parse_simple_mode with the shared parsed_options); `scale=`
    /// comes from the section ONLY (C passes `parsed_scale = 0`, so
    /// `--scale` never reaches a VNC output); the transform is forced
    /// normal (no section read at all); `resizeable=` defaults true.
    /// C pipewire_backend_output_configure: 640x480 defaults, scale
    /// from the section, transform forced NORMAL (so no transform
    /// field here), plus the section's own `gbm-format`.
    pub fn decide_pipewire(&self, head_name: &str) -> Option<PipewireOutputSetup> {
        let rule = self.rules.iter().find(|r| r.name == head_name);
        if let Some(r) = rule
            && r.off
        {
            return None;
        }
        let (mut width, mut height) = (640, 480);
        let mut scale = 1;
        let mut gbm_format = None;
        if let Some(r) = rule {
            if let Some((w, h)) = r.size {
                width = w;
                height = h;
            }
            if let Some(s) = r.scale {
                scale = s;
            }
            gbm_format = r.gbm_format.clone();
        }
        if let Some(w) = self.cli.width {
            width = w;
        }
        if let Some(h) = self.cli.height {
            height = h;
        }
        Some(PipewireOutputSetup {
            width,
            height,
            scale,
            gbm_format,
        })
    }

    pub fn decide_vnc(&self, head_name: &str) -> Option<VncOutputSetup> {
        let rule = self.rules.iter().find(|r| r.name == head_name);
        if let Some(r) = rule
            && r.off
        {
            return None;
        }
        let (mut width, mut height) = (640, 480);
        let mut scale = 1;
        let mut resizeable = true;
        if let Some(r) = rule {
            if let Some((w, h)) = r.size {
                width = w;
                height = h;
            }
            if let Some(s) = r.scale {
                scale = s;
            }
            if let Some(v) = r.resizeable {
                resizeable = v;
            }
        }
        if let Some(w) = self.cli.width {
            width = w;
        }
        if let Some(h) = self.cli.height {
            height = h;
        }
        Some(VncOutputSetup {
            width,
            height,
            scale,
            resizeable,
        })
    }

    /// Does this head's section carry `mirror-of=`?  (C
    /// wet_config_head_has_mirror_of_entry — the remote-head deferral
    /// test in simple_head_enable.)
    pub fn has_mirror_of(&self, head_name: &str) -> bool {
        self.rules
            .iter()
            .any(|r| r.name == head_name && r.mirror_of.is_some())
    }

    /// The rule (remote head) configured to mirror `source_name`, if
    /// any (C wet_config_find_head_to_mirror's section scan).
    pub fn mirror_rule_for_source(&self, source_name: &str) -> Option<&OutputRule> {
        self.rules
            .iter()
            .find(|r| r.mirror_of.as_deref() == Some(source_name))
    }
}

/// What the pipewire configure needs (C pipewire_backend_output_configure,
/// main.c:3558): parse_simple_mode defaults 640x480, `wet_output_set_scale`,
/// a per-output `gbm-format`, and the transform forced NORMAL.
#[derive(Debug, Clone, PartialEq)]
pub struct PipewireOutputSetup {
    pub width: i32,
    pub height: i32,
    pub scale: i32,
    pub gbm_format: Option<String>,
}

/// The resolved per-head decision for a VNC output (C
/// vnc_backend_output_configure: no transform — always normal).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VncOutputSetup {
    pub width: i32,
    pub height: i32,
    pub scale: i32,
    pub resizeable: bool,
}

/// C `wet_output_set_eotf_mode`'s `modes[]` table (main.c:1408),
/// names exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EotfMode {
    #[default]
    Sdr,
    HdrGamma,
    St2084,
    Hlg,
}

impl EotfMode {
    pub fn parse(s: &str) -> Option<EotfMode> {
        Some(match s {
            "sdr" => EotfMode::Sdr,
            "hdr-gamma" => EotfMode::HdrGamma,
            "st2084" => EotfMode::St2084,
            "hlg" => EotfMode::Hlg,
            _ => return None,
        })
    }

    /// C's error message lists the valid names; keep the same order.
    pub const NAMES: [&'static str; 4] = ["sdr", "hdr-gamma", "st2084", "hlg"];

    pub(crate) fn to_c(self) -> u32 {
        use weston_sys::weston_eotf_mode as m;
        match self {
            EotfMode::Sdr => m::WESTON_EOTF_MODE_SDR,
            EotfMode::HdrGamma => m::WESTON_EOTF_MODE_TRADITIONAL_HDR,
            EotfMode::St2084 => m::WESTON_EOTF_MODE_ST2084,
            EotfMode::Hlg => m::WESTON_EOTF_MODE_HLG,
        }
    }
}

/// C `wet_output_set_colorimetry_mode`'s `modes[]` (main.c:1469).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColorimetryMode {
    #[default]
    Default,
    Bt2020Cycc,
    Bt2020Ycc,
    Bt2020Rgb,
    P3d65,
    P3dci,
    Ictcp,
}

impl ColorimetryMode {
    pub fn parse(s: &str) -> Option<ColorimetryMode> {
        Some(match s {
            "default" => ColorimetryMode::Default,
            "bt2020cycc" => ColorimetryMode::Bt2020Cycc,
            "bt2020ycc" => ColorimetryMode::Bt2020Ycc,
            "bt2020rgb" => ColorimetryMode::Bt2020Rgb,
            "p3d65" => ColorimetryMode::P3d65,
            "p3dci" => ColorimetryMode::P3dci,
            "ictcp" => ColorimetryMode::Ictcp,
            _ => return None,
        })
    }

    pub const NAMES: [&'static str; 7] = [
        "default",
        "bt2020cycc",
        "bt2020ycc",
        "bt2020rgb",
        "p3d65",
        "p3dci",
        "ictcp",
    ];

    pub(crate) fn to_c(self) -> u32 {
        use weston_sys::weston_colorimetry_mode as m;
        match self {
            ColorimetryMode::Default => m::WESTON_COLORIMETRY_MODE_DEFAULT,
            ColorimetryMode::Bt2020Cycc => m::WESTON_COLORIMETRY_MODE_BT2020_CYCC,
            ColorimetryMode::Bt2020Ycc => m::WESTON_COLORIMETRY_MODE_BT2020_YCC,
            ColorimetryMode::Bt2020Rgb => m::WESTON_COLORIMETRY_MODE_BT2020_RGB,
            ColorimetryMode::P3d65 => m::WESTON_COLORIMETRY_MODE_P3D65,
            ColorimetryMode::P3dci => m::WESTON_COLORIMETRY_MODE_P3DCI,
            ColorimetryMode::Ictcp => m::WESTON_COLORIMETRY_MODE_ICTCP,
        }
    }
}

/// One `[[color-characteristics]]` block, already parsed and
/// **group-validated** by the frontend (C `parse_color_characteristics`,
/// main.c:1538).
///
/// The grouping rule is the whole point and is easy to miss: the eleven
/// keys form five groups — primaries (the six red/green/blue x,y),
/// white (x,y), max luminance, min luminance, maxFALL — and each group
/// must be given **entirely or not at all**.  Half a group is a config
/// error, not a partial application.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ColorCharacteristics {
    pub primaries: Option<[(f32, f32); 3]>,
    pub white: Option<(f32, f32)>,
    pub max_luminance: Option<f32>,
    pub min_luminance: Option<f32>,
    pub max_fall: Option<f32>,
}

impl ColorCharacteristics {
    pub(crate) fn group_mask(&self) -> u32 {
        use weston_sys::weston_color_characteristics_groups as g;
        let mut mask = 0;
        if self.primaries.is_some() {
            mask |= g::WESTON_COLOR_CHARACTERISTICS_GROUP_PRIMARIES;
        }
        if self.white.is_some() {
            mask |= g::WESTON_COLOR_CHARACTERISTICS_GROUP_WHITE;
        }
        if self.max_luminance.is_some() {
            mask |= g::WESTON_COLOR_CHARACTERISTICS_GROUP_MAXL;
        }
        if self.min_luminance.is_some() {
            mask |= g::WESTON_COLOR_CHARACTERISTICS_GROUP_MINL;
        }
        if self.max_fall.is_some() {
            mask |= g::WESTON_COLOR_CHARACTERISTICS_GROUP_MAXFALL;
        }
        mask
    }
}

/// The colour slice of an output's configuration — read by every
/// backend's configure for `icc-profile`/`allow-hdcp`, and by the DRM
/// and headless ones for the rest (C main.c:1873/2415-2423/3459-3464).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ColorSetup {
    /// `icc-profile=` (C's ini key is the underscored `icc_profile`).
    /// Only consulted when the colour manager is loaded.
    pub icc_profile: Option<String>,
    pub eotf_mode: Option<EotfMode>,
    pub colorimetry_mode: Option<ColorimetryMode>,
    pub characteristics: Option<ColorCharacteristics>,
    /// `allow-hdcp=` (C's `allow_hdcp`), default true.
    pub allow_hdcp: bool,
}

/// How a DRM output picks its video mode (C
/// `weston_drm_backend_output_mode` plus the modeline string that goes
/// with it, main.c:2356-2378).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum DrmMode {
    /// `mode=preferred`, and the default when the key is absent.
    #[default]
    Preferred,
    /// `mode=current`, or `--current-mode` for every output.
    Current,
    /// Anything else is handed to the backend verbatim as a modeline —
    /// "1280x720", "1280x720@60", or a full modeline.  C does no
    /// validation of its own here; the backend reports the failure.
    Modeline(String),
}

/// The resolved per-head decision for a DRM output — C
/// `drm_backend_output_configure` (main.c:2333), which reads a wider
/// slice of the section than any other backend's configure.
// No `Eq`: the colour characteristics are f32, so equality is
// partial by construction.
#[derive(Debug, Clone, PartialEq)]
pub struct DrmOutputSetup {
    pub mode: DrmMode,
    /// `max-bpc=`, C default 16.  Note the interaction: with
    /// `mode=current` and no explicit `max-bpc`, C passes 0 instead, so
    /// that reusing the current mode does not force a full modeset.
    pub max_bpc: u32,
    pub scale: i32,
    /// `transform=`.  `None` means "no section value" — the caller
    /// substitutes the *head's* own transform, which C reads at
    /// configure time and this layer cannot see.
    pub transform: Option<OutputTransform>,
    pub gbm_format: Option<String>,
    pub content_type: Option<String>,
    /// `seat=`, C default the empty string (not absent — C always calls
    /// set_seat, with "" when the key is missing).
    pub seat: String,
    /// The colour slice, taken from the *controlling* section — DRM is
    /// the one path where clone-of has already redirected which section
    /// applies (C reads all of these from `lo->section`).
    pub color: ColorSetup,
}

/// Why a DRM head is not getting an output.
/// The payload of [`DrmHeadPlan::Enable`], boxed inside the enum.
#[derive(Debug, Clone, PartialEq)]
pub struct DrmEnablePlan {
    pub output_name: String,
    pub setup: DrmOutputSetup,
}

// No `Eq`, for the same reason as DrmOutputSetup above.  The Enable
// variant is boxed because it is far larger than the others and clippy
// is right that every DrmHeadPlan would otherwise pay for it.
#[derive(Debug, Clone, PartialEq)]
pub enum DrmHeadPlan {
    /// Enable it, on the layoutput named by the section's `name=` (or
    /// by the head itself when no section matches).
    Enable(Box<DrmEnablePlan>),
    /// `mode=off`: pruned before it ever reaches a layoutput
    /// (drm_head_prepare_enable, main.c:2777).
    Off,
    /// A non-desktop head whose controlling section has no `mode=`
    /// key — C's "non-desktop and not explicitly enabled" skip
    /// (main.c:2779-2781); the backend turns the head off.  A
    /// non-desktop head with NO section at all is *staged* in C
    /// (main.c:2792-2794) and therefore returns `Enable` here.
    NonDesktop,
    /// `clone-of` pointed at a missing section, or nested too deep.
    /// C logs and returns NULL, which lands the head in the
    /// no-section branch; we keep it distinct so the frontend can log
    /// C's exact message once and then treat it as no-section.
    BadCloneOf(String),
}

impl OutputPolicy {
    /// Resolve the section that *controls* a head, following `clone-of`
    /// (C `drm_config_find_controlling_output_section`, main.c:2436).
    ///
    /// A section carrying `clone-of=X` contributes nothing of its own:
    /// every setting comes from X's section instead, recursively.  C
    /// bounds the recursion at depth 10 and logs two distinct
    /// configuration errors; both are reproduced.
    fn drm_controlling_rule(&self, head_name: &str) -> Result<Option<&OutputRule>, String> {
        let mut want = head_name.to_string();
        let mut depth = 0;
        loop {
            let Some(rule) = self.rules.iter().find(|r| r.name == want) else {
                if depth > 0 {
                    return Err(format!(
                        "Configuration error: output section referred to with \
                         'clone-of={want}' not found."
                    ));
                }
                return Ok(None);
            };
            depth += 1;
            if depth > 10 {
                return Err(format!(
                    "Configuration error: 'clone-of' nested too deep for output '{head_name}'."
                ));
            }
            match &rule.clone_of {
                Some(next) => want = next.clone(),
                None => return Ok(Some(rule)),
            }
        }
    }

    /// C `drm_head_prepare_enable` + `drm_backend_output_configure`, as
    /// one decision.  `current_mode_cli` is `--current-mode`, which
    /// forces every output to `mode=current` regardless of its section.
    pub fn decide_drm(
        &self,
        head_name: &str,
        non_desktop: bool,
        current_mode_cli: bool,
    ) -> DrmHeadPlan {
        let rule = match self.drm_controlling_rule(head_name) {
            Ok(r) => r,
            Err(message) => return DrmHeadPlan::BadCloneOf(message),
        };
        let Some(rule) = rule else {
            // No section: C stages the head under its own name, with
            // every default, non-desktop or not (main.c:2792-2794 —
            // the `else` branch of drm_head_prepare_enable has no
            // non_desktop check; the skip at 2780 belongs to the
            // has-section arm below).  Preserved to the letter: the
            // oracle is 14.0.1, and quietly skipping here would change
            // which connectors get outputs on real hardware.
            return DrmHeadPlan::Enable(Box::new(DrmEnablePlan {
                output_name: head_name.to_string(),
                setup: self.drm_setup(None, current_mode_cli),
            }));
        };
        if rule.off {
            return DrmHeadPlan::Off;
        }
        // C main.c:2779-2781: "non-desktop and not explicitly enabled"
        // — a controlling section WITHOUT a mode= key does not count as
        // explicitly enabling a non-desktop head (merely setting
        // scale= on an HMD must not light it up); any mode= value that
        // is not "off" does.
        if non_desktop && rule.mode_string.is_none() {
            return DrmHeadPlan::NonDesktop;
        }
        // C keys the layoutput on the *section's* name=, which is how
        // several heads land on one output (clone mode).
        DrmHeadPlan::Enable(Box::new(DrmEnablePlan {
            output_name: rule.name.clone(),
            setup: self.drm_setup(Some(rule), current_mode_cli),
        }))
    }

    fn drm_setup(&self, rule: Option<&OutputRule>, current_mode_cli: bool) -> DrmOutputSetup {
        let mode = match rule.and_then(|r| r.mode_string.as_deref()) {
            _ if current_mode_cli => DrmMode::Current,
            None | Some("preferred") => DrmMode::Preferred,
            Some("current") => DrmMode::Current,
            Some(other) => DrmMode::Modeline(other.to_string()),
        };
        // C: max-bpc defaults to 16, EXCEPT that mode=current with no
        // explicit max-bpc passes 0 so no full modeset is done
        // (main.c:2364).
        let max_bpc = match rule.and_then(|r| r.max_bpc) {
            Some(v) => v,
            None if mode == DrmMode::Current => 0,
            None => 16,
        };
        DrmOutputSetup {
            mode,
            max_bpc,
            // C wet_output_set_scale(output, section, 1, 0): default 1,
            // and `0` for the CLI value means --scale never reaches a
            // DRM output.
            scale: rule.and_then(|r| r.scale).unwrap_or(1),
            transform: rule.and_then(|r| r.transform),
            gbm_format: rule.and_then(|r| r.gbm_format.clone()),
            content_type: rule.and_then(|r| r.content_type.clone()),
            seat: rule.and_then(|r| r.seat.clone()).unwrap_or_default(),
            color: rule.map(|r| r.color.clone()).unwrap_or_default(),
        }
    }

    /// The colour slice for a head on a *simple* (non-DRM) backend.
    /// C reads `icc_profile` and `allow_hdcp` there and nothing else
    /// (simple_head_enable, main.c:1873/1881); the eotf/colorimetry/
    /// characteristics keys are DRM-path only, so a value set on a
    /// headless or VNC output would be inert — the frontend refuses
    /// that rather than ignoring it.
    pub fn decide_color(&self, head_name: &str) -> ColorSetup {
        self.rules
            .iter()
            .find(|r| r.name == head_name)
            .map(|r| r.color.clone())
            .unwrap_or_default()
    }

    /// The setup a DRM output gets with no controlling section at all —
    /// every C default.  Used when a layoutput outlives the head that
    /// named it, so there is no rule left to consult.
    pub fn drm_defaults(&self, current_mode_cli: bool) -> DrmOutputSetup {
        self.drm_setup(None, current_mode_cli)
    }

    /// C `drm_head_should_force_enable` (main.c:2799): `force-on=true`
    /// enables a head even when the connector reads disconnected.
    /// Read from the *controlling* section, as C does.
    pub fn drm_force_on(&self, head_name: &str) -> bool {
        matches!(self.drm_controlling_rule(head_name), Ok(Some(r)) if r.force_on)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn precedence_defaults_section_cli() {
        let mut p = OutputPolicy::defaults(1024, 640);
        assert_eq!(
            p.decide("headless").unwrap(),
            OutputSetup {
                width: 1024,
                height: 640,
                scale: 1,
                transform: OutputTransform::Normal
            }
        );
        p.rules.push(OutputRule {
            name: "headless".into(),
            off: false,
            size: Some((640, 480)),
            scale: Some(2),
            transform: Some(OutputTransform::Rotate90),
            resizeable: None,
            gbm_format: None,
            mirror_of: None,
            ..Default::default()
        });
        assert_eq!(
            p.decide("headless").unwrap(),
            OutputSetup {
                width: 640,
                height: 480,
                scale: 2,
                transform: OutputTransform::Rotate90
            }
        );
        // A section for another head is inert (C name-matched lookup).
        assert_eq!(p.decide("X1").unwrap().width, 1024);
        // CLI wins over the section.
        p.cli.width = Some(800);
        p.cli.scale = Some(1);
        let s = p.decide("headless").unwrap();
        assert_eq!((s.width, s.height, s.scale), (800, 480, 1));
        assert_eq!(s.transform, OutputTransform::Rotate90);
    }

    #[test]
    fn off_rule_disables_only_its_head() {
        let mut p = OutputPolicy::defaults(1024, 640);
        p.rules.push(OutputRule {
            name: "X1".into(),
            off: true,
            ..OutputRule::default()
        });
        assert!(p.decide("X1").is_none());
        assert!(p.decide("headless").is_some());
    }

    #[test]
    fn vnc_decision_matches_c_configure() {
        let mut p = OutputPolicy::defaults(1024, 640);
        // C vnc defaults, regardless of the windowed defaults above.
        let s = p.decide_vnc("vnc").unwrap();
        assert_eq!(
            s,
            VncOutputSetup {
                width: 640,
                height: 480,
                scale: 1,
                resizeable: true
            }
        );
        p.rules.push(OutputRule {
            name: "vnc".into(),
            size: Some((800, 500)),
            scale: Some(2),
            transform: Some(OutputTransform::Rotate90), // ignored for vnc
            resizeable: Some(false),
            gbm_format: None,
            ..OutputRule::default()
        });
        // CLI --scale must NOT reach a VNC output (C parsed_scale = 0);
        // CLI --width/--height must.
        p.cli.scale = Some(3);
        p.cli.width = Some(1000);
        let s = p.decide_vnc("vnc").unwrap();
        assert_eq!(
            s,
            VncOutputSetup {
                width: 1000,
                height: 500,
                scale: 2,
                resizeable: false
            }
        );
        // off applies to vnc heads too.
        p.rules[0].off = true;
        assert!(p.decide_vnc("vnc").is_none());
    }

    #[test]
    fn mirror_lookups_match_c_scan() {
        let mut p = OutputPolicy::defaults(1024, 640);
        p.rules.push(OutputRule {
            name: "vnc".into(),
            mirror_of: Some("headless".into()),
            ..OutputRule::default()
        });
        p.rules.push(OutputRule {
            name: "X1".into(),
            ..OutputRule::default()
        });
        assert!(p.has_mirror_of("vnc"));
        assert!(!p.has_mirror_of("X1"));
        assert!(!p.has_mirror_of("headless"));
        assert_eq!(
            p.mirror_rule_for_source("headless")
                .map(|r| r.name.as_str()),
            Some("vnc")
        );
        assert!(p.mirror_rule_for_source("vnc").is_none());
        // A mirrored head's own decide_vnc still answers (the enable
        // path forces resizeable off separately, as C's configure does).
        assert!(p.decide_vnc("vnc").is_some());
    }

    #[test]
    fn transform_grammar_matches_c() {
        for (s, t) in [
            ("normal", OutputTransform::Normal),
            ("rotate-90", OutputTransform::Rotate90),
            ("rotate-180", OutputTransform::Rotate180),
            ("rotate-270", OutputTransform::Rotate270),
            ("flipped", OutputTransform::Flipped),
            ("flipped-rotate-90", OutputTransform::FlippedRotate90),
            ("flipped-rotate-180", OutputTransform::FlippedRotate180),
            ("flipped-rotate-270", OutputTransform::FlippedRotate270),
        ] {
            assert_eq!(OutputTransform::parse(s), Some(t));
        }
        assert_eq!(OutputTransform::parse("rotate-45"), None);
    }

    /// C `drm_head_prepare_enable`'s non-desktop matrix, to the letter
    /// (main.c:2764-2796).  vkms never reports non-desktop heads, so
    /// the e2e suite cannot pin these four branches — this test is the
    /// only guard until the real-hardware R3 validation.
    #[test]
    fn drm_non_desktop_matrix_matches_c() {
        let mut p = OutputPolicy::defaults(1024, 640);
        p.rules.push(OutputRule {
            name: "HDMI-A-1".into(),
            // A section that merely tweaks the head, with no mode= key:
            // "not explicitly enabled".
            scale: Some(2),
            ..OutputRule::default()
        });
        p.rules.push(OutputRule {
            name: "DP-1".into(),
            mode_string: Some("preferred".into()),
            ..OutputRule::default()
        });
        p.rules.push(OutputRule {
            name: "DP-2".into(),
            off: true,
            mode_string: Some("off".into()),
            ..OutputRule::default()
        });

        let is_enable = |plan: &DrmHeadPlan| matches!(plan, DrmHeadPlan::Enable(_));

        // No section at all: staged regardless of non-desktop
        // (main.c:2792-2794 — the else branch has no non_desktop check).
        assert!(is_enable(&p.decide_drm("eDP-1", true, false)));
        assert!(is_enable(&p.decide_drm("eDP-1", false, false)));

        // Section without mode=: skipped for a non-desktop head
        // (main.c:2779-2781), staged for a desktop one.
        assert_eq!(
            p.decide_drm("HDMI-A-1", true, false),
            DrmHeadPlan::NonDesktop
        );
        assert!(is_enable(&p.decide_drm("HDMI-A-1", false, false)));

        // Section with a non-off mode=: "explicitly enabled" — staged
        // even when non-desktop.
        assert!(is_enable(&p.decide_drm("DP-1", true, false)));

        // mode=off wins over everything, desktop or not.
        assert_eq!(p.decide_drm("DP-2", true, false), DrmHeadPlan::Off);
        assert_eq!(p.decide_drm("DP-2", false, false), DrmHeadPlan::Off);
    }
}
