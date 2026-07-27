"""Backend/output configuration (plan §2.3): CLI geometry, [output]
sections, and multi-backend, observed via wayland-info and the VNC
framebuffer."""

import os
import re
import subprocess

import pytest

from support.compositor import CONFIG_FORMAT

WESTONITE = os.environ.get("WESTONITE_BIN", "westonite")

# `off`/`mode=off` on a windowed backend is an R2b re-spec extension:
# C's parse_simple_mode has no per-output off switch and treats the
# spelling as an unparseable mode, so only the Rust frontend can be
# asked to assert it.
toml_only = pytest.mark.skipif(
    CONFIG_FORMAT != "toml",
    reason="re-spec extension (R2b): Rust frontend only")

OUTPUT_GLOBAL = "interface: 'wl_output'"


def wayland_info(w):
    return w.run_client(["wayland-info"]).stdout


def output_block(info):
    """The wl_output interface section of wayland-info output."""
    match = re.search(r"interface: 'wl_output'.*?(?=\ninterface: |\Z)",
                      info, re.S)
    assert match, f"no wl_output in wayland-info output:\n{info}"
    return match.group(0)


def run_to_exit(argv, tmp_path, timeout=15):
    """Run westonite to completion for invocations that must fail during
    startup; returns (returncode, log text)."""
    runtime = tmp_path / "xdg"
    runtime.mkdir(mode=0o700, exist_ok=True)
    log = tmp_path / "startup.log"
    env = dict(os.environ)
    env["XDG_RUNTIME_DIR"] = str(runtime)
    env.pop("WAYLAND_DISPLAY", None)
    proc = subprocess.run(
        [WESTONITE, "--backend=headless", f"--log={log}", "--no-config"]
        + list(argv),
        env=env, timeout=timeout, capture_output=True, text=True)
    text = log.read_text(errors="replace") if log.exists() else ""
    return proc.returncode, text


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


def test_output_transform_from_cli(westonite):
    w = westonite(extra_args=["--transform=rotate-270"])
    block = output_block(wayland_info(w))
    assert re.search(r"transform:\s*270\b", block), block


def test_invalid_transform_is_fatal(tmp_path):
    code, log = run_to_exit(["--transform=bogus"], tmp_path)
    assert code != 0, log
    assert 'Invalid transform "bogus"' in log, log


def test_invalid_mode_falls_back_to_defaults(westonite):
    """C parse_simple_mode: an unparseable mode logs and keeps the
    backend defaults rather than failing startup."""
    w = westonite(width=None,
                  config="[output]\nname=headless\nmode=1024xNOPE\n")
    w.wait_for_log(r"Invalid mode for output headless\. Using defaults\.")
    block = output_block(wayland_info(w))
    assert re.search(r"width:\s*1024\s*px,\s*height:\s*640\s*px", block), block


def test_no_outputs_advertises_no_wl_output(westonite):
    w = westonite(extra_args=["--no-outputs"])
    info = wayland_info(w)
    assert OUTPUT_GLOBAL not in info, info


@toml_only
def test_output_off_from_config_leaves_head_unenabled(westonite):
    w = westonite(width=None, config="[output]\nname=headless\noff=true\n")
    w.wait_for_log(r"output headless disabled by config")
    info = wayland_info(w)
    assert OUTPUT_GLOBAL not in info, info


@toml_only
def test_output_off_is_scoped_to_its_own_head(westonite):
    """A section naming a head that never appears is inert -- the
    headless output still comes up (C's name-matched section lookup)."""
    w = westonite(width=None, config="[output]\nname=DP-1\noff=true\n")
    block = output_block(wayland_info(w))
    assert re.search(r"width:\s*1024\s*px,\s*height:\s*640\s*px", block), block


@toml_only
def test_vnc_mirrors_headless_output(westonite):
    """mirror-of (R2c-mirror): the VNC output mirrors the headless one,
    coming up at the SOURCE's mode (1024x640), not VNC's 640x480
    default, with client resize disabled.

    Rust frontend only: the C oracle ABORTS on this exact config —
    headless never populates native_mode_copy and main.c:2543 asserts
    on it (verified live); our frontend falls back to the source's
    current mode instead (see apply_mirror_modeline)."""
    w = westonite(backend="vnc", width=None,
                  extra_args=["--backends=headless,vnc"],
                  config="[output]\nname=vnc\nmirror-of=headless\n")
    w.wait_for_log(r"Use of mirror_of disables resizing for output vnc")
    w.wait_for_log(r"Setting modeline to output 'vnc' to 1024x640, scale: 1")
    with w.vnc() as vnc:
        assert (vnc.width, vnc.height) == (1024, 640)
    # The mirror is enabled from inside the source's output-created
    # signal, i.e. nested in the very heads-changed flush that still has
    # the vnc head left to visit -- C resets the head's device-changed
    # flag there (wet_output_handle_create) precisely so that pass stays
    # quiet.  By now the flush is long over (a client has connected).
    assert "Detected a monitor change" not in w.log(), w.log()


@toml_only
def test_vnc_mirror_defers_until_source_exists(westonite):
    """With vnc listed FIRST, its head appears before the headless
    output exists: the mirror must defer (C simple_head_enable's
    remote-mirror early return) and still come up once the source's
    output-created signal fires."""
    w = westonite(backend="vnc", width=None,
                  extra_args=["--backends=vnc,headless"],
                  config="[output]\nname=vnc\nmirror-of=headless\n")
    with w.vnc() as vnc:
        assert (vnc.width, vnc.height) == (1024, 640)


@toml_only
def test_mirror_of_without_remote_backend_is_fatal(westonite):
    # fail-loud: mirror-of needs a remote (vnc) backend loaded
    w = westonite(config="[output]\nname=vnc\nmirror-of=headless\n",
                  wait=False)
    w.wait_for_log(r"mirror-of requires a remote backend")
    assert w.wait_exit() != 0


def test_vnc_output_mode_from_config(westonite):
    w = westonite(backend="vnc", width=None,
                  config="[output]\nname=vnc\nmode=800x500\n")
    with w.vnc() as vnc:
        assert (vnc.width, vnc.height) == (800, 500)


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
    from support.compositor import wait_until

    w = westonite(backend="vnc", extra_args=["--backends=headless,vnc"])

    def both_outputs_advertised():
        names = re.findall(r"name:\s*(\S+)", wayland_info(w))
        return len([n for n in names if n in ("headless", "vnc")]) == 2

    # outputs from the second backend can come up after the socket does
    wait_until(both_outputs_advertised,
               message="both headless and vnc outputs to be advertised")
    with w.vnc() as vnc:
        vnc.capture()
