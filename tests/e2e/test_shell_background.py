"""desktop-shell built-in background (plan §2.4): the shell draws one
solid-color curtain per output; color comes from [shell]
background-color, default 0xff002244."""

import pytest

from support.compositor import CONFIG_NAME
from support.image import wait_for_solid_color


@pytest.mark.installed
def test_background_default(westonite):
    w = westonite(backend="vnc")
    with w.vnc() as vnc:
        assert (vnc.width, vnc.height) == (640, 480)
        wait_for_solid_color(vnc, (0x00, 0x22, 0x44))


@pytest.mark.installed
def test_background_from_config(westonite):
    w = westonite(backend="vnc",
                  config="[shell]\nbackground-color=0xff336699\n")
    with w.vnc() as vnc:
        wait_for_solid_color(vnc, (0x33, 0x66, 0x99))
    # the color came from the config file westonite looked up (P2
    # discovery; westonite.ini for the C frontend, westonite.toml since
    # the R2a re-spec)
    assert CONFIG_NAME in w.log()
