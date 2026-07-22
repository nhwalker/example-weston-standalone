"""Xwayland integration (plan §2.5): lazy server spawn, X11 windows
rendering through the shell, and X<->compositor position sync.

X windows come from wtest-xclient (tests/e2e/clients/) -- EL10 ships no
X demo apps, and a flat-color window allows exact pixel assertions.
"""

import os

import pytest

from support.client import WtestClient
from support.compositor import wait_until
from support.image import pick_pixel, region_of, wait_for_region

MAGENTA = (0xCC, 0x00, 0xCC)  # cc00cc
ORANGE = (0xCC, 0x66, 0x00)   # cc6600


def make_xclient(w, *args):
    return WtestClient(w, *args,
                       binary=os.environ.get("WTEST_XCLIENT",
                                             "wtest-xclient"),
                       extra_env={"DISPLAY": w.x_display})


def last_position(xc):
    reports = [line for line in xc.output() if line.startswith("position: ")]
    if not reports:
        return None
    parts = reports[-1].split()
    return int(parts[1]), int(parts[2])


def titlebar_drag(vnc, x, y, dx, dy):
    """Press-hold-drag on an xwm titlebar. xwm processes the X-side
    button press asynchronously, so the button must stay held while the
    grab gets set up."""
    import time
    vnc.pointer(x, y, 0)
    vnc.pointer(x, y, 1)
    time.sleep(0.5)
    for i in range(1, 13):
        vnc.pointer(x + dx * i // 12, y + dy * i // 12, 1)
        time.sleep(0.03)
    time.sleep(0.3)
    vnc.pointer(x + dx, y + dy, 0)


@pytest.mark.installed
def test_lazy_spawn_and_roundtrip(westonite):
    w = westonite(extra_args=["--xwayland"])
    display = w.x_display
    # advertised does not mean spawned: the server starts lazily on the
    # first X connection
    assert "launching '/usr/bin/Xwayland'" not in w.log()

    result = w.run_client(["xdpyinfo", "-display", display])
    assert "vendor string" in result.stdout
    w.wait_for_log(r"launching '/usr/bin/Xwayland'")


def test_x11_window_renders_and_position_syncs(westonite):
    w = westonite(backend="vnc", extra_args=["--xwayland"])
    with w.vnc() as vnc, make_xclient(w, "--color", "cc00cc") as xc:
        xc.wait_for_line(r"^mapped$")
        box, _ = wait_for_region(vnc, MAGENTA)
        assert (box[2], box[3]) == (200, 150)

        # the position the X server reports must converge on where the
        # compositor actually shows the content (xwm position sync).
        # Recapture inside the poll: placement can still shift the
        # window shortly after the first frame, and the client reports
        # earlier pre-placement positions, so only latest-vs-current
        # counts.
        def in_sync():
            _, _, fb = vnc.capture()
            current = region_of(fb, vnc.width, MAGENTA)
            return current and last_position(xc) == (current[0], current[1])

        wait_until(in_sync,
                   message="X-reported position to match the content box")


def test_x11_window_drag_updates_x_position(westonite):
    w = westonite(backend="vnc", extra_args=["--xwayland"])
    with w.vnc() as vnc, make_xclient(w, "--color", "cc00cc") as xc:
        xc.wait_for_line(r"^mapped$")
        box, _ = wait_for_region(vnc, MAGENTA)
        x, y = box[0], box[1]

        # drag the xwm-drawn titlebar (a few pixels above the content);
        # the shell's move grab relocates the window and xwm must push
        # the new position back to the X client
        titlebar_drag(vnc, x + box[2] // 2, y - 10, 80, 50)

        def synced():
            _, _, fb = vnc.capture()
            new = region_of(fb, vnc.width, MAGENTA)
            if not new or (new[0], new[1]) == (x, y):
                return False
            # last reported X position matches the on-screen content
            return last_position(xc) == (new[0], new[1])

        wait_until(synced, message="X position to follow the moved window")


def test_two_x11_clients(westonite):
    w = westonite(backend="vnc", extra_args=["--xwayland"])
    with w.vnc() as vnc, \
         make_xclient(w, "--color", "cc00cc") as one, \
         make_xclient(w, "--color", "cc6600") as two:
        one.wait_for_line(r"^mapped$")
        two.wait_for_line(r"^mapped$")
        _, fb = wait_for_region(vnc, MAGENTA)
        assert pick_pixel(fb, vnc.width, ORANGE) or \
            wait_for_region(vnc, ORANGE)