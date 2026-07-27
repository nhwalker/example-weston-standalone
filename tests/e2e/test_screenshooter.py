"""Screenshooter + wcap recorder (plan §4, R2e): Super+S spawns the
weston-screenshooter client, gated by an in-flight-client slot and a
capture-authority listener that authorizes only that client; Super+R
toggles the wcap recorder.

Key events go in over VNC as QEMU extended key events (keycodes): the
server's keysym path deliberately skips xkb modifier tracking for
Super (RFC6143 shift-state rules; only Ctrl/Alt get
STATE_UPDATE_AUTOMATIC in vnc_handle_key_event), so Super+X bindings
are reachable only through vnc_handle_key_code_event.

Capture COMPLETION is not testable here: ANY weston-output-capture
attempt -- authorized or denied alike -- ABORTS the compositor inside
the weston-libs RPM once a VNC peer is connected: vnc_output_repaint
only reaches the renderer through neatvnc's aml dispatch, so a capture
task can survive the repaint cycle and trip
`assert(wl_list_empty(&ci->pending_capture_list))`
(output-capture.c:193, weston 14.0.1; reproduced with the C frontend,
both for a Super+S-authorized shot and for a denied foreign client).
RPM-side bug, out of scope per the our-code-only decision -- see
docs/e2e-test-plan.md §6.  The spawn path is therefore diverted with
WESTON_MODULE_MAP (both frontends resolve the binary through
weston_module_path_from_env) so the binding/gate/spawn machinery is
verified without a capture ever being committed, and the
authority-denial test runs on headless, where the deny is clean
server-side.  (The abort on the authorized path is itself proof the
authority listener authorized the spawned client.)
"""

import struct
import time

from support.compositor import wait_until

# (keysym, qnum keycode) pairs -- neatvnc maps the wire keycode through
# code_map_qnum_to_linux (qnum 0xdb -> KEY_LEFTMETA).
SUPER_L = (0xffeb, 0xdb)
KEY_S = (0x073, 0x1f)
KEY_R = (0x072, 0x13)

WCAP_MAGIC = 0x57434150


def super_tap(vnc, key):
    vnc.key_code(*SUPER_L, True)
    vnc.key_code(*key, True)
    vnc.key_code(*key, False)
    vnc.key_code(*SUPER_L, False)


def test_super_s_spawns_screenshooter_and_slot_recycles(westonite, tmp_path):
    """Super+S resolves the client through WESTON_MODULE_MAP and spawns
    it; once that attempt is over (the diverted path cannot exec, so
    the child/client dies immediately) the in-flight slot frees and a
    second Super+S spawns again."""
    exe = tmp_path / "absent-screenshooter"  # never created
    w = westonite(backend="vnc",
                  env={"WESTON_MODULE_MAP": f"weston-screenshooter={exe}"})
    with w.vnc() as vnc:
        vnc.capture()
        super_tap(vnc, KEY_S)
        w.wait_for_log(rf"launching '{exe}'")

        def second_spawn_logged():
            # keep tapping: the exact moment the slot frees is an
            # implementation detail (C holds it until the doomed
            # wl_client dies; the Rust frontend fails the spawn before
            # a client exists)
            super_tap(vnc, KEY_S)
            return w.log().count(f"launching '{exe}'") >= 2

        wait_until(second_spawn_logged, message="slot to free and respawn")


def test_foreign_capture_client_is_denied(westonite):
    """A weston-screenshooter the compositor did NOT spawn gets no
    capture (only the Super+S child is authorized) and the compositor
    survives the attempt.  Headless: with a VNC peer connected even a
    DENIED attempt trips the RPM-side repaint assert (see module
    docstring), while the headless deny is clean server-side -- the
    stock client then dies on its own zero-size-buffer assert, which
    is the observable denial."""
    w = westonite()
    result = w.run_client(["/usr/bin/weston-screenshooter"], check=False)
    assert result.returncode != 0, result
    assert w.proc.poll() is None


def test_super_r_toggles_wcap_recorder(westonite):
    w = westonite(backend="vnc")
    with w.vnc() as vnc:
        vnc.capture()
        wcap = w.workdir / "capture.wcap"

        super_tap(vnc, KEY_R)
        wait_until(wcap.exists, message="capture.wcap to appear")

        # force some damage so frames get recorded
        for x in range(0, 200, 20):
            vnc.pointer(x, 100, 0)
            time.sleep(0.05)

        super_tap(vnc, KEY_R)  # stop

        def stopped_with_frames():
            data = wcap.read_bytes()
            if len(data) < 16:
                return False
            magic, fmt, width, height = struct.unpack("<IIII", data[:16])
            return (magic == WCAP_MAGIC
                    and (width, height) == (vnc.width, vnc.height)
                    and len(data) > 16)

        wait_until(stopped_with_frames,
                   message="stopped wcap file with header + frames")
