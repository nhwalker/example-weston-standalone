"""desktop-shell window management via VNC + wtest-client (plan §2.4).

Every scene is flat-colored, so window geometry is read straight out of
the framebuffer with exact color matching. Colors are chosen unique per
window (and per focus state) within each test.
"""

import time

from support.client import WtestClient
from support.compositor import wait_until
from support.image import pick_pixel, region_of, wait_for_region

RED = (0xCC, 0x00, 0x00)          # ffcc0000
BRIGHT_RED = (0xFF, 0x44, 0x44)   # ffff4444
BLUE = (0x00, 0x00, 0xCC)         # ff0000cc
BRIGHT_BLUE = (0x44, 0x44, 0xFF)  # ff4444ff
GREEN = (0x00, 0xCC, 0x00)        # ff00cc00


def make_client(w, *args):
    return WtestClient(w, *args)


def test_window_maps_inside_output(westonite):
    w = westonite(backend="vnc")
    with w.vnc() as vnc, make_client(w, "--color", "ffcc0000") as client:
        assert client.wait_mapped() == (200, 150)
        box, _ = wait_for_region(vnc, RED)
        x, y, bw, bh = box
        assert (bw, bh) == (200, 150), f"visible region {box}"
        assert x >= 0 and y >= 0
        assert x + bw <= 640 and y + bh <= 480


def test_new_window_gets_keyboard_focus(westonite):
    w = westonite(backend="vnc")
    with w.vnc() as vnc, \
         make_client(w, "--color", "ffcc0000",
                     "--focus-color", "ffff4444") as client:
        client.wait_for_line(r"focus: enter")
        wait_for_region(vnc, BRIGHT_RED)


def test_click_moves_activation_between_windows(westonite):
    w = westonite(backend="vnc")
    with w.vnc() as vnc, \
         make_client(w, "--color", "ffcc0000",
                     "--focus-color", "ffff4444", "--title", "A") as a, \
         make_client(w, "--color", "ff0000cc",
                     "--focus-color", "ff4444ff", "--title", "B") as b:
        a.wait_mapped()
        b.wait_mapped()
        # B mapped last -> B holds focus
        b.wait_for_line(r"focus: enter")
        box_a, fb = wait_for_region(vnc, RED)  # A unfocused

        # click a guaranteed-visible pixel of A -> focus follows the click
        click_x, click_y = pick_pixel(fb, vnc.width, RED)
        vnc.click(click_x, click_y)
        a.wait_for_line(r"focus: enter")
        b.wait_for_line(r"focus: leave")
        wait_for_region(vnc, BRIGHT_RED)   # A now drawn focused
        wait_for_region(vnc, BLUE)         # B drawn unfocused

        # keys land in A now
        vnc.key_tap(0x0061)  # 'a'
        a.wait_for_line(r"^key: ")


def grab_drag(vnc, client, x0, y0, x1, y1, button=1, steps=8):
    """Press, wait until the client saw the button (so its move/resize
    request reached the shell while the button is still down), then
    drag and release."""
    mask = 1 << (button - 1)
    n = client.count(r"pointer: button")
    vnc.pointer(x0, y0, 0)
    vnc.pointer(x0, y0, mask)
    client.wait_for_count(r"pointer: button", n + 1)
    for i in range(1, steps + 1):
        vnc.pointer(x0 + (x1 - x0) * i // steps,
                    y0 + (y1 - y0) * i // steps, mask)
    vnc.pointer(x1, y1, 0)


def move_window_to(vnc, client, client_box, target_xy):
    """Left-drag an --interactive window so its origin lands on target."""
    x, y, bw, bh = client_box
    grab_drag(vnc, client, x + bw // 2, y + bh // 2,
              target_xy[0] + bw // 2, target_xy[1] + bh // 2)


def test_pointer_move_grab(westonite):
    w = westonite(backend="vnc")
    with w.vnc() as vnc, \
         make_client(w, "--color", "ff00cc00", "--interactive") as client:
        client.wait_mapped()
        box, _ = wait_for_region(vnc, GREEN)

        # left press makes the client request xdg_toplevel.move; the
        # shell's pointer move grab must then follow the drag exactly
        target = (60, 60)
        move_window_to(vnc, client, box, target)

        def moved():
            _, _, fb = vnc.capture()
            new = region_of(fb, vnc.width, GREEN)
            return new and (new[0], new[1]) == target and \
                (new[2], new[3]) == (box[2], box[3])

        wait_until(moved, message=f"window to land at {target}")


def test_pointer_resize_grab(westonite):
    w = westonite(backend="vnc")
    with w.vnc() as vnc, \
         make_client(w, "--color", "ff00cc00", "--interactive") as client:
        client.wait_mapped()
        box, _ = wait_for_region(vnc, GREEN)
        bw, bh = box[2], box[3]

        # park the window at a known spot so the corner drag stays
        # inside the output (initial placement is randomized)
        move_window_to(vnc, client, box, (40, 40))
        wait_until(lambda: region_of(vnc.capture()[2], vnc.width, GREEN)
                   and region_of(vnc.capture()[2], vnc.width, GREEN)[:2]
                   == (40, 40), message="window parked at (40, 40)")

        # right press inside the bottom-right corner: the client
        # requests xdg_toplevel.resize(bottom-right); dragging must grow
        # the window, with pixels, configure, and commit agreeing.
        #
        # The exact grow amount is deliberately not asserted: the RPM's
        # VNC backend translates drag motions with an odd half-delta,
        # skip-events pattern (docs/e2e-test-plan.md §6) while our
        # shell's resize grab faithfully tracks whatever positions it
        # is handed -- asserting exact deltas would test the RPM's
        # input translation, not our code.
        start = (40 + bw - 5, 40 + bh - 5)
        grab_drag(vnc, client, start[0], start[1],
                  start[0] + 80, start[1] + 60, button=3)

        match = client.wait_for_line(r"configure: ([1-9]\d*)x(\d+) \[ resizing")
        new_w, new_h = int(match.group(1)), int(match.group(2))
        assert new_w > bw and new_h > bh, \
            f"resize went {bw}x{bh} -> {new_w}x{new_h}"

        def grew_consistently():
            _, _, fb = vnc.capture()
            new = region_of(fb, vnc.width, GREEN)
            if not new or new[2] <= bw or new[3] <= bh:
                return False
            # last sized configure == what is actually on screen
            last = [line for line in client.output()
                    if line.startswith("configure:") and "0x0" not in line][-1]
            return last.startswith(f"configure: {new[2]}x{new[3]}")

        wait_until(grew_consistently,
                   message="window pixels to grow and match the configure")


def test_wm_capabilities_empty(westonite):
    # T6/T9: the shell advertises no window-state requests at all
    w = westonite(backend="vnc")
    with w.vnc(), make_client(w) as client:
        match = client.wait_for_line(r"wm-capabilities: \[(.*)\]")
        assert match.group(1).strip() == "", (
            f"expected empty wm_capabilities, got {match.group(0)!r}")


def test_fullscreen_request_ignored(westonite):
    w = westonite(backend="vnc")
    with w.vnc() as vnc, \
         make_client(w, "--color", "ffcc0000",
                     "--request-fullscreen") as client:
        assert client.wait_mapped() == (200, 150)
        box, _ = wait_for_region(vnc, RED)
        assert (box[2], box[3]) == (200, 150)
        assert not any("fullscreen" in line
                       for line in client.output()
                       if line.startswith("configure:"))


def test_maximize_request_ignored(westonite):
    w = westonite(backend="vnc")
    with w.vnc() as vnc, \
         make_client(w, "--color", "ffcc0000",
                     "--request-maximized") as client:
        assert client.wait_mapped() == (200, 150)
        box, _ = wait_for_region(vnc, RED)
        assert (box[2], box[3]) == (200, 150)
        assert not any("maximized" in line
                       for line in client.output()
                       if line.startswith("configure:"))


def test_background_clicks_are_swallowed(westonite):
    w = westonite(backend="vnc")
    with w.vnc() as vnc, \
         make_client(w, "--color", "ffcc0000",
                     "--focus-color", "ffff4444") as client:
        client.wait_for_line(r"focus: enter")
        box, _ = wait_for_region(vnc, BRIGHT_RED)

        # click far away from the window, on the bare background
        bg_x = 5 if box[0] > 100 else 630
        vnc.click(bg_x, 470)
        time.sleep(0.5)
        # focus must not leave the window; it stays drawn focused
        assert not any(line == "focus: leave" for line in client.output())
        _, _, fb = vnc.capture()
        assert region_of(fb, vnc.width, BRIGHT_RED)


def test_unresponsive_client_handled(westonite):
    # T8: clicking an unresponsive window must not start grabs or hurt
    # the compositor; the client recovers on resume
    w = westonite(backend="vnc")
    with w.vnc() as vnc, \
         make_client(w, "--color", "ff00cc00", "--interactive") as client:
        client.wait_mapped()
        box, _ = wait_for_region(vnc, GREEN)
        x, y, bw, bh = box

        client.pause()
        for _ in range(3):
            vnc.click(x + bw // 2, y + bh // 2)
            time.sleep(0.2)
        # window cannot have moved: the paused client never issued
        # xdg_toplevel.move, and T8 removed move-on-busy-click
        _, _, fb = vnc.capture()
        assert region_of(fb, vnc.width, GREEN)[:2] == (x, y)

        client.resume()
        # client comes back: events flow again (e.g. keys reach it)
        vnc.click(x + bw // 2, y + bh // 2)
        client.wait_for_line(r"pointer: button")
        assert client.proc.poll() is None
