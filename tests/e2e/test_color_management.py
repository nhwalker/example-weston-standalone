"""Colour management (plan §7 R2c): `[core] color-management` and the
`[[output]]` colour keys.

Most of this is *validation*, and validation is exactly what a port
gets wrong quietly, so most of these tests assert startup errors.  All
of them run in the container, no VM needed: config validation happens
before any backend is loaded, so a case can request `--backend=drm`
purely to get past the "these keys are drm-only" gate and still fail on
the error under test.  The parts that need a real KMS output -- what a
connector actually *supports* -- live in `test_backend_drm.py`.

The rules being enforced come from C:

  - `wet_output_set_eotf_mode` / `_set_colorimetry_mode` (main.c:1403,
    1464) reject unknown names and require `color-management=true` for
    any non-default mode.
  - `parse_color_characteristics` (main.c:1538) range-checks every
    value and requires each *group* to be given entirely or not at all.
  - `wet_output_set_color_profile` (main.c:1365) returns before it even
    reads `icc_profile` when the manager is absent — so C ignores an
    icc-profile silently.  We refuse instead; that divergence is the
    point of `test_icc_profile_without_color_management_is_refused`.
"""

import pytest

from support.compositor import CONFIG_FORMAT

# These are Rust-frontend rules: C either accepts silently or has no
# equivalent spelling for the TOML-only keys.
toml_only = pytest.mark.skipif(
    CONFIG_FORMAT != "toml",
    reason="colour-management validation is a Rust-frontend re-spec")

pytestmark = toml_only


def fails_with(westonite, config, pattern, backend="headless"):
    """Start with `config` and assert it is a startup error matching
    `pattern`.  Every case in this file has that shape.

    `backend="drm"` for the cases whose validation only *applies* to a
    DRM output: the "these keys are drm-only" refusal is checked first,
    so on a headless output it would mask the very error under test.
    Requesting DRM in the container is fine and does not need the VM —
    config validation runs before any backend is loaded, so the
    startup error arrives long before DRM would fail to open a device.
    """
    w = westonite(backend=backend, config=config, wait=False)
    w.wait_for_log(pattern)
    assert w.wait_exit() != 0, w.log()


def test_unknown_eotf_mode_is_refused(westonite):
    fails_with(westonite, """
[output]
name=headless
eotf-mode=not-a-mode
""", r"not a valid EOTF mode", backend="drm")


def test_unknown_colorimetry_mode_is_refused(westonite):
    fails_with(westonite, """
[output]
name=headless
colorimetry-mode=not-a-mode
""", r"not a valid colorimetry mode", backend="drm")


def test_eotf_mode_on_a_non_drm_output_is_refused(westonite):
    """eotf/colorimetry/characteristics reach libweston only through the
    DRM configure, so on a headless output they would be inert."""
    fails_with(westonite, """
[output]
name=headless
eotf-mode=sdr
""", r"apply to the drm backend only")


def test_icc_profile_without_color_management_is_refused(westonite):
    """C returns before reading icc_profile when the colour manager is
    absent, so it ignores this silently.  Fail-loud instead."""
    fails_with(westonite, """
[output]
name=headless
icc-profile=/nonexistent.icc
""", r"icc-profile requires \[core\] color-management")


def test_half_a_characteristics_group_is_refused(westonite):
    """C's rule: each group entirely or not at all.  Three of the six
    primaries is a config error, not a partial application."""
    fails_with(westonite, """
[core]
color-management=true

[output]
name=headless
color-characteristics=cc1

[color_characteristics]
name=cc1
red-x=0.68
red-y=0.32
green-x=0.265
""", r"You must set either none or all keys of a group", backend="drm")


def test_out_of_range_chromaticity_is_refused(westonite):
    """Chromaticity coordinates are 0..=1 in C's table."""
    fails_with(westonite, """
[core]
color-management=true

[output]
name=headless
color-characteristics=cc1

[color_characteristics]
name=cc1
red-x=1.5
red-y=0.32
green-x=0.265
green-y=0.69
blue-x=0.15
blue-y=0.06
""", r"is outside of the range", backend="drm")


def test_missing_characteristics_section_is_refused(westonite):
    fails_with(westonite, """
[core]
color-management=true

[output]
name=headless
color-characteristics=nosuch
""", r"no \[\[color-characteristics\]\] section", backend="drm")


def test_allow_hdcp_is_accepted_on_any_backend(westonite):
    """allow_hdcp is the one colour-adjacent key C reads on *every*
    backend (allow_content_protection, main.c:1873), and it was missing
    from the config model entirely until this slice."""
    w = westonite(config="""
[output]
name=headless
allow-hdcp=false
""")
    assert "fatal" not in w.log(), w.log()


def test_vrr_mode_and_max_cll_are_unknown_keys(westonite):
    """Neither is read anywhere in weston 14.0.1 — `vrr-mode` has no
    reader in main.c and `weston_color_characteristics` has no maxCLL
    field.  They were in our config model, where they would have been
    silently accepted and silently ignored; now they are unknown keys,
    which `deny_unknown_fields` reports with a span (D9)."""
    fails_with(westonite, """
[output]
name=headless
vrr-mode=game
""", r"unknown field|vrr-mode")
