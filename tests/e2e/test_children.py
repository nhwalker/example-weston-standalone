"""Child processes: helper-client policy (P3) and autolaunch (plan §2.2).

Stub clients are tiny shell scripts that record their environment to a
file; "spawned" is asserted by that file appearing.
"""

import pytest

import signal
import time

from support.compositor import wait_until


def make_stub(tmp_path, name="stub", body="sleep 300"):
    """An executable that dumps its environment, then runs `body`."""
    marker = tmp_path / f"{name}.env"
    script = tmp_path / name
    script.write_text(f"#!/bin/sh\nenv > {marker}\n{body}\n")
    script.chmod(0o755)
    return script, marker


def wait_for_file(path, deadline=10.0):
    return wait_until(path.exists, deadline=deadline,
                      message=f"{path.name} to be created")


# -- helper-client policy (patch P3) ------------------------------------


@pytest.mark.installed
def test_no_helper_clients_by_default(westonite):
    w = westonite()
    w.wait_for_log(r"Loading module '.*/desktop-shell\.so'")
    time.sleep(1.0)  # grace period: a spawn would happen right at startup
    assert "launching" not in w.log()


def test_shell_client_setting_is_ignored(westonite, tmp_path):
    # T3 removed the helper-client machinery entirely: even an explicit
    # [shell] client= must spawn nothing and must not disturb startup
    stub, marker = make_stub(tmp_path)
    w = westonite(config=f"[shell]\nclient={stub}\n")
    w.wait_for_log(r"Loading module '.*/desktop-shell\.so'")
    time.sleep(1.0)
    assert not marker.exists()
    assert "launching" not in w.log()


# -- autolaunch ---------------------------------------------------------


def test_autolaunch_config_spawns(westonite, tmp_path):
    stub, marker = make_stub(tmp_path)
    w = westonite(config=f"[autolaunch]\npath={stub}\n")
    wait_for_file(marker)
    env = dict(line.split("=", 1) for line in
               marker.read_text().splitlines() if "=" in line)
    # the client speaks wayland to this compositor ...
    assert env.get("WAYLAND_DISPLAY") == w.wayland_display
    # ... and sees the config file westonite used (WESTON_CONFIG_FILE export)
    assert env.get("WESTON_CONFIG_FILE") == str(w.config_home / "westonite.ini")


@pytest.mark.installed
def test_autolaunch_watch_exits_with_client(westonite, tmp_path):
    # the kiosk primitive: watch=true ties the session to the client
    stub, marker = make_stub(tmp_path, body="sleep 1")
    w = westonite(config=f"[autolaunch]\npath={stub}\nwatch=true\n")
    wait_for_file(marker)
    code = w.wait_exit()
    assert code == 0, f"compositor exited {code}\n--- log ---\n{w.log()}"


def test_autolaunch_no_watch_survives_client_exit(westonite, tmp_path):
    stub, marker = make_stub(tmp_path, body="exit 0")
    w = westonite(config=f"[autolaunch]\npath={stub}\nwatch=false\n")
    wait_for_file(marker)
    time.sleep(1.0)
    assert w.proc.poll() is None, f"compositor died\n--- log ---\n{w.log()}"


def test_autolaunch_child_crash_tolerated(westonite, tmp_path):
    stub, marker = make_stub(tmp_path, body="kill -SEGV $$")
    w = westonite(config=f"[autolaunch]\npath={stub}\nwatch=false\n")
    wait_for_file(marker)
    time.sleep(1.0)
    assert w.proc.poll() is None, f"compositor died\n--- log ---\n{w.log()}"


def test_autolaunch_nonexecutable_is_fatal(westonite):
    w = westonite(config="[autolaunch]\npath=/nonexistent-client\n",
                  wait=False)
    w.wait_for_log(r"autolaunch path \(/nonexistent-client\) is not executable")
    assert w.wait_exit() != 0


def test_positional_command_runs_and_watch_applies(westonite, tmp_path):
    # `westonite -- CMD ARGS`: spawned with args, and the compositor
    # always watches it (exits when it exits)
    stub, marker = make_stub(tmp_path, body='[ "$1" = "arg1" ] && sleep 1')
    w = westonite(extra_args=["--", str(stub), "arg1"])
    wait_for_file(marker)
    code = w.wait_exit()
    assert code == 0, f"compositor exited {code}\n--- log ---\n{w.log()}"


def test_clean_shutdown_with_live_child(westonite, tmp_path):
    # a still-running autolaunched client must not wedge or dirty shutdown
    stub, marker = make_stub(tmp_path)
    w = westonite(config=f"[autolaunch]\npath={stub}\n")
    wait_for_file(marker)
    w.terminate(signal.SIGTERM)
