"""Frame assertions for VNC captures (4-byte BGRX pixel buffers)."""

from .compositor import wait_until


def _first_match(row, pattern):
    """Leftmost pixel index whose BGR bytes equal pattern, or None."""
    i = row.find(pattern)
    while i != -1:
        if i % 4 == 0:
            return i // 4
        i = row.find(pattern, i + 1)
    return None


def _last_match(row, pattern):
    i = row.rfind(pattern)
    while i != -1:
        if i % 4 == 0:
            return i // 4
        i = row.rfind(pattern, 0, i + len(pattern) - 1)
    return None


def pick_pixel(fb, fb_width, rgb):
    """Coordinates (x, y) of some pixel exactly matching rgb, or None.
    Unlike a bounding-box center, the returned pixel is guaranteed to
    actually show the color (safe to click even with overlap)."""
    pattern = bytes((rgb[2], rgb[1], rgb[0]))
    stride = fb_width * 4
    for y in range(len(fb) // stride):
        x = _first_match(fb[y * stride:(y + 1) * stride], pattern)
        if x is not None:
            return (x, y)
    return None


def region_of(fb, fb_width, rgb):
    """Bounding box (x, y, w, h) of pixels exactly matching rgb, or None.

    Scenes are all flat colors (shell background + wtest-client fills),
    so exact matching is safe; per-row bytes.find keeps it fast.
    """
    pattern = bytes((rgb[2], rgb[1], rgb[0]))  # BGR in the capture buffer
    stride = fb_width * 4
    xmin = ymin = xmax = ymax = None
    for y in range(len(fb) // stride):
        row = fb[y * stride:(y + 1) * stride]
        first = _first_match(row, pattern)
        if first is None:
            continue
        last = _last_match(row, pattern)
        if ymin is None:
            ymin = y
        ymax = y
        if xmin is None or first < xmin:
            xmin = first
        if xmax is None or last > xmax:
            xmax = last
    if ymin is None:
        return None
    return (xmin, ymin, xmax - xmin + 1, ymax - ymin + 1)


def wait_for_region(client, rgb, deadline=10.0):
    """Poll captures until a region of the exact color exists; return
    (bounding box, framebuffer)."""
    state = {}

    def check():
        _, _, fb = client.capture()
        box = region_of(fb, client.width, rgb)
        state.update(box=box, fb=fb)
        return box

    wait_until(check, deadline=deadline,
               message=f"a region of rgb{tuple(rgb)} to appear")
    return state["box"], state["fb"]


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
