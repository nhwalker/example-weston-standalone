"""Frame assertions for VNC captures (4-byte BGRX pixel buffers)."""

from .compositor import wait_until


def solid_color(fb, rgb):
    """True if every pixel of the BGRX buffer is exactly rgb."""
    r, g, b = rgb
    return (set(fb[0::4]) == {b}
            and set(fb[1::4]) == {g}
            and set(fb[2::4]) == {r})


def wait_for_solid_color(client, rgb, deadline=10.0):
    """Poll captures until the whole framebuffer is exactly rgb.

    The first frames after connect may predate the shell's first repaint,
    so a single capture is never asserted directly.
    """
    last = {}

    def check():
        w, h, fb = client.capture()
        last.update(w=w, h=h, fb=fb)
        return solid_color(fb, rgb)

    try:
        wait_until(check, deadline=deadline,
                   message=f"framebuffer to be solid #{bytes(rgb).hex()}")
    except AssertionError:
        seen = sorted({(last["fb"][i + 2], last["fb"][i + 1], last["fb"][i])
                       for i in range(0, len(last["fb"]), 4)})[:8]
        raise AssertionError(
            f"framebuffer never became solid rgb{tuple(rgb)}; "
            f"last frame {last['w']}x{last['h']} contained colors {seen}")
    return last["fb"]
