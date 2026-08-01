"""The DRM backend (plan §7 R2c), against a vkms device inside a VM.

These tests do not run in the ordinary suite.  They need a KMS device
they may take DRM master on, which `scripts/drm-vm-test.sh` provides by
booting a throwaway VM with `vkms` loaded -- see docs/drm-testing.md.

The gate is the WESTONITE_DRM_VM environment variable and *not* the
presence of /dev/dri: a developer running the suite on a desktop has a
/dev/dri, and taking DRM master on it would black out their display
mid-test.  Only the VM harness sets the variable.

vkms presents one connected connector named "Virtual-1" whose preferred
mode is 1024x768@60; every expectation below is anchored to that.
"""

import os
import re

import pytest

in_drm_vm = pytest.mark.skipif(
    os.environ.get("WESTONITE_DRM_VM") != "1",
    reason="needs the DRM VM harness (scripts/drm-vm-test.sh)")

pytestmark = in_drm_vm

# The connector vkms advertises, and the name weston therefore gives the
# head and the output it drives.
HEAD = "Virtual-1"
PREFERRED = (1024, 768)


def output_block(w, name=HEAD):
    """The wayland-info stanza for one wl_output, read back through the
    protocol rather than from the log.  Split on the interface header
    rather than searching forward from the name: wayland-info prints an
    output's name and its mode in an order we would rather not depend
    on."""
    info = w.run_client(["wayland-info"]).stdout
    blocks = [b for b in re.split(r"(?m)^interface:", info)
              if "'wl_output'" in b and re.search(rf"name:\s*{re.escape(name)}\b", b)]
    assert len(blocks) == 1, f"expected one output named {name}, got {len(blocks)}:\n{info}"
    return blocks[0]


def mode_of(w, name=HEAD):
    """(width, height) of an output's *current* mode.

    Unlike headless and vnc, a DRM output advertises every mode the
    connector supports -- 34 of them for vkms -- so "the first width:
    line" is the preferred mode, not the active one.  The flags are the
    only thing that says which is in use.
    """
    block = output_block(w, name)
    modes = re.findall(
        r"width:\s*(\d+) px,\s*height:\s*(\d+) px[^\n]*\n\s*flags:([^\n]*)",
        block)
    assert modes, f"no modes for {name} in:\n{block}"
    current = [m for m in modes if "current" in m[2]]
    assert len(current) == 1, f"expected one current mode, got {current}\n{block}"
    return int(current[0][0]), int(current[0][1])


def test_drm_backend_enables_the_connected_head(westonite):
    """drm_heads_changed enables every connected head, and the output
    takes the connector's name (main.c:3006)."""
    w = westonite(backend="drm")
    w.wait_for_log(rf"DRM: head '{HEAD}' found, connector \d+ is connected")
    w.wait_for_log(rf"Output '{HEAD}' enabled with head\(s\) {HEAD}")
    info = w.run_client(["wayland-info"]).stdout
    assert HEAD in info, info


def test_drm_output_defaults_to_the_preferred_mode(westonite):
    """drm_backend_output_configure's default is mode=preferred
    (main.c:2357), which for vkms is 1024x768."""
    w = westonite(backend="drm")
    w.wait_for_log(rf"Output '{HEAD}' enabled with head")
    assert mode_of(w) == PREFERRED


def test_drm_output_mode_from_config_is_a_modeline(westonite):
    """Anything that is neither "preferred" nor "current" is handed to
    the DRM output API as a modeline (main.c:2371).  1280x720 is in
    vkms's list, so it must be picked over the preferred mode."""
    w = westonite(backend="drm", config=f"""
[output]
name={HEAD}
mode=1280x720
""")
    w.wait_for_log(rf"Output '{HEAD}' enabled with head")
    assert mode_of(w) == (1280, 720)


def test_drm_output_scale_applies(westonite):
    """wet_output_set_scale is shared machinery, but the DRM path is the
    only one that reaches it with a head-derived transform in play
    (main.c:2384) -- worth one check that it still lands."""
    w = westonite(backend="drm", config=f"""
[output]
name={HEAD}
scale=2
""")
    w.wait_for_log(rf"Output '{HEAD}' enabled with head")
    assert re.search(r"scale:\s*2", output_block(w)), output_block(w)


def test_drm_output_off_leaves_no_outputs_and_that_is_fatal(westonite):
    """mode=off prunes the head before it ever reaches a layoutput
    (drm_head_prepare_enable, main.c:2777).  vkms has exactly one head,
    so pruning it leaves the layoutput list empty -- and the default
    require-outputs=any then fails startup (main.c:2977) rather than
    running blind.  Fail-loud, and the "off was supposed to be pruned"
    assert further down drm_backend_output_configure is never reached
    (hitting it would be a crash, not an error)."""
    w = westonite(backend="drm", config=f"""
[output]
name={HEAD}
mode=off
""", wait=False)
    assert w.wait_exit() != 0, w.log()
    assert "was supposed to be pruned" not in w.log()


def test_drm_device_can_be_selected(westonite):
    """--drm-device names the card to use (main.c:3394)."""
    w = westonite(backend="drm", extra_args=["--drm-device=card0"])
    w.wait_for_log(r"using /dev/dri/card0")
    w.wait_for_log(rf"Output '{HEAD}' enabled with head")


def test_unknown_drm_device_is_a_startup_error(westonite):
    """A device that does not exist must fail loudly at startup, not
    silently fall back to whatever card happens to be first."""
    w = westonite(backend="drm", extra_args=["--drm-device=card99"],
                  wait=False)
    assert w.wait_exit() != 0, w.log()


# ---------------------------------------------------------------------
# [libinput] -- the DRM backend's configure_device hook (main.c:2239).
#
# This is the only backend that has the hook, so this is the only place
# the section can be tested at all.  The harness attaches a virtio
# keyboard and a virtio mouse for exactly this (scripts/drm-vm-test.sh);
# q35 also brings PS/2 emulations and an ACPI power button along, which
# is why every assertion below is per named device rather than over the
# whole log.
# ---------------------------------------------------------------------

KEYBOARD = "QEMU Virtio Keyboard"
MOUSE = "QEMU Virtio Mouse"


def configured_devices(w):
    """Every device the hook ran for, in call order."""
    return re.findall(r'libinput: configuring device "([^"]*)"', w.log())


def libinput_block(w, device):
    """The settings configure_input_device logged for one device.

    C prints each applied key indented under the device's "configuring
    device" line, so the block runs from that line to the next log line
    that is not one of them.  Returned in log order: the order is C's
    fixed sequence through the function, and pinning it is free.
    """
    block, collecting = [], False
    for line in w.log().splitlines():
        named = re.search(r'libinput: configuring device "([^"]*)"', line)
        if named:
            collecting = named.group(1) == device
            continue
        if not collecting:
            continue
        # C's ten-space indent, after the timestamp the C frontend
        # prefixes and the Rust one does not.  Exactly ten, because
        # weston's own multi-line continuations are indented fifteen --
        # the width of that timestamp -- and one of those following a
        # device's last setting would otherwise be swallowed into the
        # block.
        setting = re.match(r"^(?:\[[^\]]*\] )? {10}(\S.*)$", line)
        if setting:
            block.append(setting.group(1))
        else:
            collecting = False
    return block


def test_libinput_hook_runs_for_every_device(westonite):
    """C installs configure_device unconditionally, so the per-device
    line appears even with no [libinput] section -- and with no section
    the hook must touch nothing."""
    w = westonite(backend="drm")
    w.wait_for_log(rf"Output '{HEAD}' enabled with head")
    names = configured_devices(w)
    assert KEYBOARD in names, names
    assert MOUSE in names, names
    assert libinput_block(w, MOUSE) == []
    assert libinput_block(w, KEYBOARD) == []


def test_libinput_pointer_settings_apply(westonite):
    """Every pointer-side key the mouse supports, in C's order."""
    w = westonite(backend="drm", config="""
[libinput]
left-handed=true
middle-button-emulation=true
accel-profile=flat
accel-speed=0.5
natural-scroll=true
scroll-method=button
scroll-button=BTN_RIGHT
""")
    w.wait_for_log(rf"Output '{HEAD}' enabled with head")
    assert libinput_block(w, MOUSE) == [
        "middle-button-emulation=true",
        "left-handed=true",
        "accel-profile=flat",
        "accel-speed=0.500",
        "natural-scroll=true",
        "scroll-method=button",
        "scroll-button=BTN_RIGHT",
    ], w.log()


def test_libinput_unsupported_keys_are_skipped_per_device(westonite):
    """Capability gating, which must stay silent rather than become an
    error: enable-tap needs a tap-capable device and neither of these
    is one, while left-handed is a mouse capability the keyboard has
    not got.  "This device cannot do that" is a runtime fact, not a
    mistake in the config."""
    w = westonite(backend="drm", config="""
[libinput]
enable-tap=true
left-handed=true
""")
    w.wait_for_log(rf"Output '{HEAD}' enabled with head")
    assert libinput_block(w, MOUSE) == ["left-handed=true"], w.log()
    assert libinput_block(w, KEYBOARD) == [], w.log()
    assert "enable-tap" not in w.log()
