"""desktop-shell built-in background (plan §2.4): the shell draws one
solid-color curtain per output; color comes from [shell]
background-color, default 0xff002244."""

from support.image import wait_for_solid_color


def test_background_default(westonite):
    w = westonite(backend="vnc")
    with w.vnc() as vnc:
        assert (vnc.width, vnc.height) == (640, 480)
        wait_for_solid_color(vnc, (0x00, 0x22, 0x44))


def test_background_from_config(westonite):
    w = westonite(backend="vnc",
                  config="[shell]\nbackground-color=0xff336699\n")
    with w.vnc() as vnc:
        wait_for_solid_color(vnc, (0x33, 0x66, 0x99))
    # the color came from the file the P2 patch made westonite look up
    assert "westonite.ini" in w.log()
