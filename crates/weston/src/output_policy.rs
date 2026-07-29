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
    /// `[output] gbm-format=` (pipewire configure reads it per output;
    /// the DRM one is not ported).
    pub gbm_format: Option<String>,
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
}
