"""Nested and streaming backends (plan §7 R2c): wayland, x11, pipewire.

None of these needs a VM or a GPU.  Each runs against something we
already know how to start:

  - **wayland**: a second westonite nested inside a headless one.
  - **x11**: westonite as an X client of the Xwayland *we* spawn
    (EL10 ships no X.org server at all -- no Xvfb -- so the compositor
    under test provides the X server for the compositor under test).
  - **pipewire**: a pipewire daemon in the same runtime dir.

All three default to the GL renderer and there is no GPU here, so the
harness passes --renderer=pixman (same reason as the VNC leg).
"""

import os
import shutil
import subprocess

import pytest

from support.compositor import CONFIG_FORMAT, wait_until

# The Rust frontend drops RDP by product decision; the C oracle still
# builds it, so only the Rust binary can be asked to refuse it.
toml_only = pytest.mark.skipif(
    CONFIG_FORMAT != "toml",
    reason="RDP removal is a Rust-frontend product decision")

OUTPUT_GLOBAL = "interface: 'wl_output'"


def outputs_of(w):
    """Output names the compositor advertises, via wayland-info run as
    a client of THAT compositor."""
    info = w.run_client(["wayland-info"]).stdout
    import re
    return re.findall(r"name:\s*(\S+)", info)


def test_wayland_backend_nests_in_another_westonite(westonite):
    """C load_wayland_backend: one output per parent surface, named
    wayland0 by default (main.c:4152)."""
    host = westonite(socket_name="nest-host")
    nested = westonite(backend="wayland",
                       socket_name="nest-child",
                       runtime_dir=host.runtime_dir,
                       env={"WAYLAND_DISPLAY": "nest-host"})
    nested.wait_for_log(r"Output 'wayland0' enabled")
    assert "wayland0" in outputs_of(nested)
    # and it is a real compositor: a client can map a window on it
    client = nested.run_client(["wayland-info"])
    assert OUTPUT_GLOBAL in client.stdout


def test_wayland_backend_output_count_and_size(westonite):
    """--output-count creates waylandN heads; --width/--height size
    them (both are in C's wayland option table)."""
    host = westonite(socket_name="cnt-host")
    nested = westonite(backend="wayland",
                       socket_name="cnt-child",
                       width=800, height=500,
                       extra_args=["--output-count=2"],
                       runtime_dir=host.runtime_dir,
                       env={"WAYLAND_DISPLAY": "cnt-host"})
    nested.wait_for_log(r"Output 'wayland1' enabled")
    names = outputs_of(nested)
    assert "wayland0" in names and "wayland1" in names, names
    info = nested.run_client(["wayland-info"]).stdout
    assert "width: 800 px, height: 500 px" in info, info


def test_x11_backend_under_our_own_xwayland(westonite):
    """C load_x11_backend: weston as an X client, outputs are X windows
    named screenN (main.c:4013)."""
    host = westonite(extra_args=["--xwayland"], socket_name="x-host")
    display = host.x_display
    nested = westonite(backend="x11",
                       socket_name="x-child",
                       runtime_dir=host.runtime_dir,
                       env={"DISPLAY": display})
    nested.wait_for_log(r"Output 'screen0' enabled")
    assert "screen0" in outputs_of(nested)


def test_x11_backend_default_size_is_1024x600(westonite):
    """x11_backend_output_configure's defaults differ from every other
    backend's (1024x600, main.c:3910) -- the one place the per-backend
    default actually shows."""
    host = westonite(extra_args=["--xwayland"], socket_name="xd-host")
    nested = westonite(backend="x11", socket_name="xd-child",
                       width=None,
                       runtime_dir=host.runtime_dir,
                       env={"DISPLAY": host.x_display})
    nested.wait_for_log(r"Output 'screen0' enabled")
    info = nested.run_client(["wayland-info"]).stdout
    assert "width: 1024 px, height: 600 px" in info, info


@pytest.mark.skipif(not shutil.which("pipewire"),
                    reason="pipewire daemon not installed")
def test_pipewire_backend_publishes_an_output(westonite, tmp_path):
    """C load_pipewire_backend: num-outputs heads, configured by
    pipewire_backend_output_configure (640x480 defaults)."""
    runtime = tmp_path / "pw-xdg"
    runtime.mkdir(mode=0o700, exist_ok=True)
    env = dict(os.environ)
    env["XDG_RUNTIME_DIR"] = str(runtime)
    daemon = subprocess.Popen(["pipewire"], env=env,
                              stdout=subprocess.DEVNULL,
                              stderr=subprocess.DEVNULL)
    try:
        wait_until(lambda: (runtime / "pipewire-0").exists(),
                   message="pipewire socket")
        w = westonite(backend="pipewire", socket_name="pw-child",
                      width=None, runtime_dir=runtime)
        w.wait_for_log(r"Output 'pipewire' enabled")
        assert "pipewire" in outputs_of(w)
    finally:
        daemon.terminate()
        daemon.wait(10)


@toml_only
def test_rdp_backend_is_refused_by_design(westonite):
    """RDP is deliberately not shipped (product decision, not a gap):
    asking for it is a startup error that says so."""
    w = westonite(backend="rdp", wait=False)
    w.wait_for_log(r"rdp backend is not supported by westonite")
    assert w.wait_exit() != 0
