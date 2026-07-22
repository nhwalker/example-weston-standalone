"""Backend/output configuration (plan §2.3): CLI geometry, [output]
sections, and multi-backend, observed via wayland-info and the VNC
framebuffer."""

import re


def wayland_info(w):
    return w.run_client(["wayland-info"]).stdout


def output_block(info):
    """The wl_output interface section of wayland-info output."""
    match = re.search(r"interface: 'wl_output'.*?(?=\ninterface: |\Z)",
                      info, re.S)
    assert match, f"no wl_output in wayland-info output:\n{info}"
    return match.group(0)


def test_headless_geometry_from_cli(westonite):
    w = westonite(width=800, height=500)
    block = output_block(wayland_info(w))
    assert re.search(r"width:\s*800\s*px,\s*height:\s*500\s*px", block), block


def test_output_scale_and_transform_from_config(westonite):
    w = westonite(width=None,
                  config="[output]\nname=headless\n"
                         "mode=640x480\nscale=2\ntransform=rotate-90\n")
    block = output_block(wayland_info(w))
    assert re.search(r"scale:\s*2\b", block), block
    assert re.search(r"transform:\s*90\b", block), block


def test_vnc_output_mode_from_config(westonite):
    w = westonite(backend="vnc", width=None,
                  config="[output]\nname=vnc\nmode=800x500\n")
    with w.vnc() as vnc:
        assert (vnc.width, vnc.height) == (800, 500)


import pytest


@pytest.mark.skip(reason=(
    "client-initiated resize (SetDesktopSize) segfaults the EPEL 10 "
    "weston-libs 14.0.1 / neatvnc 0.9.0 VNC stack (crash in neatvnc's "
    "raw encoder worker, regardless of [output] resizeable) -- RPM-side "
    "bug, out of scope per the our-code-only decision; see "
    "docs/e2e-test-plan.md §6. Re-enable when EPEL ships a fixed stack."))
def test_vnc_client_resize_repaints_background(westonite):
    from support.compositor import wait_until
    from support.image import solid_color

    w = westonite(backend="vnc")
    with w.vnc() as vnc:
        vnc.capture()
        vnc.set_desktop_size(900, 600)

        def resized_and_painted():
            width, height, fb = vnc.capture()
            return ((width, height) == (900, 600)
                    and solid_color(fb, (0x00, 0x22, 0x44)))

        wait_until(resized_and_painted,
                   message="900x600 fully-painted framebuffer")


def test_multi_backend_headless_plus_vnc(westonite):
    w = westonite(backend="vnc", extra_args=["--backends=headless,vnc"])
    info = wayland_info(w)
    names = re.findall(r"name:\s*(\S+)", info)
    assert len([n for n in names if n in ("headless", "vnc")]) == 2, info
    with w.vnc() as vnc:
        vnc.capture()
