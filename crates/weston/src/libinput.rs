//! `[libinput]` device configuration (C main.c
//! `configure_input_device` + its `_accel` / `_scroll` helpers,
//! main.c:2131-2330).
//!
//! Two halves, deliberately split:
//!
//! * [`InputConfig`] and the `parse` functions are pure data.  The
//!   frontend resolves the section once at startup, so an unknown
//!   `accel-profile` / `scroll-method` / `scroll-button` is a startup
//!   error rather than C's per-device `weston_log("warning: ...")` that
//!   leaves the key silently inert.
//! * [`tramp_configure_device`] is the `weston_drm_backend_config
//!   .configure_device` hook.  C calls it once per libinput device, on
//!   its own schedule — during backend load for the devices already
//!   present, and later for hotplugs — so it is a full trampoline
//!   (panic barrier, `Ctx::current`, A4 depth wrap).
//!
//! Capability gating stays exactly as C has it: a device that cannot
//! tap, cannot rotate, has no natural scroll, is skipped in silence.
//! That is a per-device runtime fact, not a request the user got wrong,
//! so it must not become an error.
//!
//! The DRM backend is the only caller: `configure_device` lives in
//! `weston_drm_backend_config` and nowhere else, which is why every
//! other backend leaves the section inert (in C too).

use std::ffi::{CStr, CString};

use crate::ctx::Ctx;
use crate::log;
use crate::panic_barrier;

/// `libinput_config_accel_profile`, restricted to the two names C's
/// grammar accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccelProfile {
    Flat,
    Adaptive,
}

impl AccelProfile {
    pub const NAMES: [&'static str; 2] = ["flat", "adaptive"];

    pub fn parse(s: &str) -> Option<AccelProfile> {
        Some(match s {
            "flat" => AccelProfile::Flat,
            "adaptive" => AccelProfile::Adaptive,
            _ => return None,
        })
    }

    fn to_c(self) -> weston_sys::libinput_config_accel_profile::Type {
        use weston_sys::libinput_config_accel_profile as p;
        match self {
            AccelProfile::Flat => p::LIBINPUT_CONFIG_ACCEL_PROFILE_FLAT,
            AccelProfile::Adaptive => p::LIBINPUT_CONFIG_ACCEL_PROFILE_ADAPTIVE,
        }
    }

    fn name(self) -> &'static str {
        match self {
            AccelProfile::Flat => "flat",
            AccelProfile::Adaptive => "adaptive",
        }
    }
}

/// `libinput_config_scroll_method`, C's four names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollMethod {
    TwoFinger,
    Edge,
    Button,
    None,
}

impl ScrollMethod {
    pub const NAMES: [&'static str; 4] = ["two-finger", "edge", "button", "none"];

    pub fn parse(s: &str) -> Option<ScrollMethod> {
        Some(match s {
            "two-finger" => ScrollMethod::TwoFinger,
            "edge" => ScrollMethod::Edge,
            "button" => ScrollMethod::Button,
            "none" => ScrollMethod::None,
            _ => return None,
        })
    }

    fn to_c(self) -> weston_sys::libinput_config_scroll_method::Type {
        use weston_sys::libinput_config_scroll_method as m;
        match self {
            ScrollMethod::TwoFinger => m::LIBINPUT_CONFIG_SCROLL_2FG,
            ScrollMethod::Edge => m::LIBINPUT_CONFIG_SCROLL_EDGE,
            ScrollMethod::Button => m::LIBINPUT_CONFIG_SCROLL_ON_BUTTON_DOWN,
            ScrollMethod::None => m::LIBINPUT_CONFIG_SCROLL_NO_SCROLL,
        }
    }

    fn name(self) -> &'static str {
        match self {
            ScrollMethod::TwoFinger => "two-finger",
            ScrollMethod::Edge => "edge",
            ScrollMethod::Button => "button",
            ScrollMethod::None => "none",
        }
    }
}

/// A resolved `scroll-button`: the configured name is kept because C
/// echoes it back in the log line, not the code it resolved to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScrollButton {
    pub name: String,
    pub code: u32,
}

impl ScrollButton {
    /// Resolve a button name (`BTN_RIGHT`, `BTN_MIDDLE`, ...) to its
    /// evdev code exactly as C does — `libevdev_event_code_from_name`
    /// over `EV_KEY`.  libevdev takes *names* only: a bare number is
    /// not a valid spelling here, in C either.
    pub fn parse(name: &str) -> Option<ScrollButton> {
        let c = CString::new(name).ok()?;
        // SAFETY: libevdev's lookup reads the NUL-terminated name and
        // returns -1 when it is not a known EV_KEY code; `c` outlives
        // the call and libevdev keeps no reference to it.
        let code =
            unsafe { weston_sys::libevdev_event_code_from_name(weston_sys::EV_KEY, c.as_ptr()) };
        Some(ScrollButton {
            name: name.to_string(),
            code: u32::try_from(code).ok()?,
        })
    }
}

/// The `[libinput]` section, already parsed.  `None` means "key absent"
/// — C keys every one of these off `weston_config_section_get_*() == 0`
/// (present) rather than off a default value, so an absent key leaves
/// libinput's own default in place.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct InputConfig {
    pub enable_tap: Option<bool>,
    pub tap_and_drag: Option<bool>,
    pub tap_and_drag_lock: Option<bool>,
    pub disable_while_typing: Option<bool>,
    pub middle_button_emulation: Option<bool>,
    pub left_handed: Option<bool>,
    pub rotation: Option<u32>,
    pub accel_profile: Option<AccelProfile>,
    /// C ignores anything outside -1.0..=1.0; the frontend rejects it
    /// at startup instead, so by here it is in range.
    pub accel_speed: Option<f64>,
    pub natural_scroll: Option<bool>,
    pub scroll_method: Option<ScrollMethod>,
    /// Already resolved by [`ScrollButton::parse`].  Only consulted
    /// when `scroll_method` is [`ScrollMethod::Button`].
    pub scroll_button: Option<ScrollButton>,
}

/// The `configure_device` hook itself.  C installs it unconditionally
/// (main.c:3425), so the per-device "configuring device" line appears
/// on every DRM start whether or not the section exists — matched here.
pub(crate) unsafe extern "C" fn tramp_configure_device(
    _compositor: *mut weston_sys::weston_compositor,
    device: *mut weston_sys::libinput_device,
) {
    panic_barrier::guard("drm.configure_device", || {
        let Some(ctx) = Ctx::current() else { return };
        ctx.with_depth(|| {
            // Clone the Rc out and drop the borrow before any FFI call
            // (§3f): libinput's setters do not re-enter us, but the
            // discipline is uniform across trampolines.
            let cfg = ctx.inner.input_config.borrow().clone();
            if device.is_null() {
                return;
            }
            apply(&Device(device), &cfg);
        });
    });
}

/// A safe view over the libinput device C lends us for the duration of
/// the `configure_device` callback.
///
/// Every method below is one libinput device-config accessor.  They all
/// share one safety argument, stated once here: the pointer is the live
/// device C passed in, it stays valid for the whole callback, and
/// libinput borrows it for the call only — it stores no reference to
/// anything we pass and returns no pointer we outlive.  The
/// per-method comments point back at this.
struct Device(*mut weston_sys::libinput_device);

impl Device {
    fn name(&self) -> String {
        // SAFETY: see Device.  The returned string is libinput-owned
        // and copied out before the borrow ends.
        unsafe {
            let p = weston_sys::libinput_device_get_name(self.0);
            if p.is_null() {
                String::new()
            } else {
                CStr::from_ptr(p).to_string_lossy().into_owned()
            }
        }
    }

    fn tap_finger_count(&self) -> i32 {
        // SAFETY: see Device.
        unsafe { weston_sys::libinput_device_config_tap_get_finger_count(self.0) }
    }

    fn set_tap_enabled(&self, v: bool) {
        use weston_sys::libinput_config_tap_state as s;
        let v = if v {
            s::LIBINPUT_CONFIG_TAP_ENABLED
        } else {
            s::LIBINPUT_CONFIG_TAP_DISABLED
        };
        // SAFETY: see Device.
        unsafe { weston_sys::libinput_device_config_tap_set_enabled(self.0, v) };
    }

    fn set_tap_drag_enabled(&self, v: bool) {
        use weston_sys::libinput_config_drag_state as s;
        let v = if v {
            s::LIBINPUT_CONFIG_DRAG_ENABLED
        } else {
            s::LIBINPUT_CONFIG_DRAG_DISABLED
        };
        // SAFETY: see Device.
        unsafe { weston_sys::libinput_device_config_tap_set_drag_enabled(self.0, v) };
    }

    fn set_tap_drag_lock_enabled(&self, v: bool) {
        use weston_sys::libinput_config_drag_lock_state as s;
        let v = if v {
            s::LIBINPUT_CONFIG_DRAG_LOCK_ENABLED
        } else {
            s::LIBINPUT_CONFIG_DRAG_LOCK_DISABLED
        };
        // SAFETY: see Device.
        unsafe { weston_sys::libinput_device_config_tap_set_drag_lock_enabled(self.0, v) };
    }

    fn dwt_available(&self) -> bool {
        // SAFETY: see Device.
        unsafe { weston_sys::libinput_device_config_dwt_is_available(self.0) != 0 }
    }

    fn set_dwt_enabled(&self, v: bool) {
        use weston_sys::libinput_config_dwt_state as s;
        let v = if v {
            s::LIBINPUT_CONFIG_DWT_ENABLED
        } else {
            s::LIBINPUT_CONFIG_DWT_DISABLED
        };
        // SAFETY: see Device.
        unsafe { weston_sys::libinput_device_config_dwt_set_enabled(self.0, v) };
    }

    fn middle_emulation_available(&self) -> bool {
        // SAFETY: see Device.
        unsafe { weston_sys::libinput_device_config_middle_emulation_is_available(self.0) != 0 }
    }

    fn set_middle_emulation_enabled(&self, v: bool) {
        use weston_sys::libinput_config_middle_emulation_state as s;
        let v = if v {
            s::LIBINPUT_CONFIG_MIDDLE_EMULATION_ENABLED
        } else {
            s::LIBINPUT_CONFIG_MIDDLE_EMULATION_DISABLED
        };
        // SAFETY: see Device.
        unsafe { weston_sys::libinput_device_config_middle_emulation_set_enabled(self.0, v) };
    }

    fn left_handed_available(&self) -> bool {
        // SAFETY: see Device.
        unsafe { weston_sys::libinput_device_config_left_handed_is_available(self.0) != 0 }
    }

    fn set_left_handed(&self, v: bool) {
        // SAFETY: see Device.
        unsafe { weston_sys::libinput_device_config_left_handed_set(self.0, i32::from(v)) };
    }

    fn rotation_available(&self) -> bool {
        // SAFETY: see Device.
        unsafe { weston_sys::libinput_device_config_rotation_is_available(self.0) != 0 }
    }

    fn set_rotation_angle(&self, degrees: u32) {
        // SAFETY: see Device.
        unsafe { weston_sys::libinput_device_config_rotation_set_angle(self.0, degrees) };
    }

    fn accel_available(&self) -> bool {
        // SAFETY: see Device.
        unsafe { weston_sys::libinput_device_config_accel_is_available(self.0) != 0 }
    }

    fn accel_profiles(&self) -> u32 {
        // SAFETY: see Device.
        unsafe { weston_sys::libinput_device_config_accel_get_profiles(self.0) }
    }

    fn set_accel_profile(&self, p: AccelProfile) {
        // SAFETY: see Device.
        unsafe { weston_sys::libinput_device_config_accel_set_profile(self.0, p.to_c()) };
    }

    fn set_accel_speed(&self, speed: f64) {
        // SAFETY: see Device.
        unsafe { weston_sys::libinput_device_config_accel_set_speed(self.0, speed) };
    }

    fn has_natural_scroll(&self) -> bool {
        // SAFETY: see Device.
        unsafe { weston_sys::libinput_device_config_scroll_has_natural_scroll(self.0) != 0 }
    }

    fn set_natural_scroll(&self, v: bool) {
        // SAFETY: see Device.
        unsafe {
            weston_sys::libinput_device_config_scroll_set_natural_scroll_enabled(
                self.0,
                i32::from(v),
            )
        };
    }

    fn scroll_methods(&self) -> u32 {
        // SAFETY: see Device.
        unsafe { weston_sys::libinput_device_config_scroll_get_methods(self.0) }
    }

    fn set_scroll_method(&self, m: ScrollMethod) {
        // SAFETY: see Device.
        unsafe { weston_sys::libinput_device_config_scroll_set_method(self.0, m.to_c()) };
    }

    fn set_scroll_button(&self, code: u32) {
        // SAFETY: see Device.
        unsafe { weston_sys::libinput_device_config_scroll_set_button(self.0, code) };
    }
}

/// C `configure_input_device`.
fn apply(dev: &Device, cfg: &InputConfig) {
    log::log_line(&format!("libinput: configuring device \"{}\".", dev.name()));

    if dev.tap_finger_count() > 0 {
        if let Some(v) = cfg.enable_tap {
            log::log_line(&format!("          enable-tap={}.", bstr(v)));
            dev.set_tap_enabled(v);
        }
        if let Some(v) = cfg.tap_and_drag {
            log::log_line(&format!("          tap-and-drag={}.", bstr(v)));
            dev.set_tap_drag_enabled(v);
        }
        if let Some(v) = cfg.tap_and_drag_lock {
            log::log_line(&format!("          tap-and-drag-lock={}.", bstr(v)));
            dev.set_tap_drag_lock_enabled(v);
        }
    }

    if let Some(v) = cfg.disable_while_typing
        && dev.dwt_available()
    {
        log::log_line(&format!("          disable-while-typing={}.", bstr(v)));
        dev.set_dwt_enabled(v);
    }

    if let Some(v) = cfg.middle_button_emulation
        && dev.middle_emulation_available()
    {
        log::log_line(&format!("          middle-button-emulation={}", bstr(v)));
        dev.set_middle_emulation_enabled(v);
    }

    if let Some(v) = cfg.left_handed
        && dev.left_handed_available()
    {
        log::log_line(&format!("          left-handed={}", bstr(v)));
        dev.set_left_handed(v);
    }

    if let Some(v) = cfg.rotation
        && dev.rotation_available()
    {
        log::log_line(&format!("          rotation={v}"));
        dev.set_rotation_angle(v);
    }

    if dev.accel_available() {
        apply_accel(dev, cfg);
    }

    apply_scroll(dev, cfg);
}

/// C `configure_input_device_accel`.
fn apply_accel(dev: &Device, cfg: &InputConfig) {
    // C's capability gate: the device must advertise the profile.
    if let Some(profile) = cfg.accel_profile
        && profile.to_c() & dev.accel_profiles() != 0
    {
        log::log_line(&format!("          accel-profile={}", profile.name()));
        dev.set_accel_profile(profile);
    }

    if let Some(speed) = cfg.accel_speed {
        log::log_line(&format!("          accel-speed={speed:.3}"));
        dev.set_accel_speed(speed);
    }
}

/// C `configure_input_device_scroll`.
fn apply_scroll(dev: &Device, cfg: &InputConfig) {
    if let Some(v) = cfg.natural_scroll
        && dev.has_natural_scroll()
    {
        log::log_line(&format!("          natural-scroll={}", bstr(v)));
        dev.set_natural_scroll(v);
    }

    let Some(method) = cfg.scroll_method else {
        return;
    };
    // C: "none" is always settable; anything else must be advertised.
    if method != ScrollMethod::None && method.to_c() & dev.scroll_methods() == 0 {
        return;
    }
    log::log_line(&format!("          scroll-method={}", method.name()));
    dev.set_scroll_method(method);

    // C reads scroll-button only under scroll-method=button; the
    // frontend rejects it in any other combination, so an absent value
    // here means the key was absent too.
    if method == ScrollMethod::Button
        && let Some(button) = cfg.scroll_button.as_ref()
    {
        log::log_line(&format!("          scroll-button={}", button.name));
        dev.set_scroll_button(button.code);
    }
}

fn bstr(v: bool) -> &'static str {
    if v { "true" } else { "false" }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accel_profile_grammar_matches_c() {
        assert_eq!(AccelProfile::parse("flat"), Some(AccelProfile::Flat));
        assert_eq!(
            AccelProfile::parse("adaptive"),
            Some(AccelProfile::Adaptive)
        );
        // C's grammar has exactly two names; "none" and "custom" exist
        // in libinput but weston.ini never spelled them.
        assert_eq!(AccelProfile::parse("none"), None);
        assert_eq!(AccelProfile::parse("Flat"), None);
        assert_eq!(AccelProfile::parse(""), None);
        for n in AccelProfile::NAMES {
            assert!(AccelProfile::parse(n).is_some(), "{n}");
        }
    }

    #[test]
    fn scroll_method_grammar_matches_c() {
        assert_eq!(
            ScrollMethod::parse("two-finger"),
            Some(ScrollMethod::TwoFinger)
        );
        assert_eq!(ScrollMethod::parse("edge"), Some(ScrollMethod::Edge));
        assert_eq!(ScrollMethod::parse("button"), Some(ScrollMethod::Button));
        assert_eq!(ScrollMethod::parse("none"), Some(ScrollMethod::None));
        assert_eq!(ScrollMethod::parse("twofinger"), None);
        for n in ScrollMethod::NAMES {
            assert!(ScrollMethod::parse(n).is_some(), "{n}");
        }
    }

    #[test]
    fn scroll_button_names_resolve_through_libevdev() {
        let code = |n: &str| ScrollButton::parse(n).map(|b| b.code);
        // BTN_RIGHT/BTN_MIDDLE are the two the weston.ini man page
        // advertises; their evdev codes are ABI-stable.
        assert_eq!(code("BTN_RIGHT"), Some(0x111));
        assert_eq!(code("BTN_MIDDLE"), Some(0x112));
        assert_eq!(code("BTN_LEFT"), Some(0x110));
        // Not a name -> no code.  A bare number is not a name.
        assert_eq!(code("nonsense"), None);
        assert_eq!(code("273"), None);
        // NUL-bearing input cannot reach C at all.
        assert_eq!(code("BTN_\0RIGHT"), None);
        // The configured spelling survives for the log line.
        assert_eq!(
            ScrollButton::parse("BTN_RIGHT").map(|b| b.name),
            Some("BTN_RIGHT".to_string())
        );
    }

    #[test]
    fn default_config_asks_for_nothing() {
        let c = InputConfig::default();
        assert!(c.enable_tap.is_none());
        assert!(c.scroll_method.is_none());
        assert!(c.accel_speed.is_none());
    }
}
