"""Frontend CLI, socket naming, and config discovery (plan §2.1).

Direct-subprocess tests (no fixture) cover invocations that must fail
or exit immediately; fixture tests cover behaviors of a running
compositor.
"""

import pytest

import os
import subprocess

from support.compositor import wait_until

WESTONITE = os.environ.get("WESTONITE_BIN", "westonite")


def run(argv, env_extra=None, drop=(), timeout=15):
    env = {k: v for k, v in os.environ.items() if k not in drop}
    env.update(env_extra or {})
    return subprocess.run(argv, env=env, timeout=timeout,
                          capture_output=True, text=True)


# -- immediate-exit invocations -----------------------------------------


def test_version():
    r = run([WESTONITE, "--version"])
    assert r.returncode == 0
    assert r.stdout.split()[0] == "westonite"


def test_help():
    r = run([WESTONITE, "--help"])
    assert r.returncode == 0
    assert "--backend" in r.stdout


def test_unknown_backend(tmp_path):
    log = tmp_path / "log"
    r = run([WESTONITE, "--backend=bogus", f"--log={log}", "--no-config"],
            env_extra={"XDG_RUNTIME_DIR": str(tmp_path)})
    assert r.returncode != 0
    assert 'unknown backend "bogus"' in log.read_text()


def test_unhandled_option_is_fatal(tmp_path):
    # leftover args without a "--" separator are rejected, not executed
    log = tmp_path / "log"
    r = run([WESTONITE, "--backend=headless", f"--log={log}", "--no-config",
             "--bogus-option"],
            env_extra={"XDG_RUNTIME_DIR": str(tmp_path)})
    assert r.returncode != 0
    assert "unhandled option: --bogus-option" in log.read_text()


def test_missing_xdg_runtime_dir_refused():
    r = run([WESTONITE, "--backend=headless", "--no-config"],
            drop={"XDG_RUNTIME_DIR"})
    assert r.returncode != 0
    assert "XDG_RUNTIME_DIR" in r.stderr


# -- socket naming ------------------------------------------------------


def test_socket_name(westonite):
    w = westonite(socket_name="wibble-0")
    assert w.wayland_display == "wibble-0"
    assert (w.runtime_dir / "wibble-0").is_socket()


def test_two_instances_share_runtime_dir(westonite, tmp_path):
    shared = tmp_path / "shared-xdg"
    shared.mkdir(mode=0o700)
    a = westonite(runtime_dir=shared)
    b = westonite(runtime_dir=shared, wait=False)
    wait_until(lambda: len(a._sockets()) >= 2,
               message="second wayland socket to appear")
    names = a._sockets()
    assert len(set(names)) == 2, f"expected two distinct sockets, got {names}"


# -- config discovery (P2) ----------------------------------------------


@pytest.mark.installed
def test_config_found_in_xdg_config_home(westonite):
    w = westonite(config="[core]\n")
    w.wait_for_log(r"Using config file '.*/westonite\.ini'")


def test_config_explicit_path(westonite, tmp_path):
    ini = tmp_path / "custom-name.ini"
    ini.write_text("[core]\n")
    w = westonite(extra_args=[f"--config={ini}"], no_config_flag=False)
    w.wait_for_log(rf"Using config file '{ini}'")


@pytest.mark.installed
def test_stock_weston_ini_is_ignored(westonite):
    # P2: only westonite.ini is searched; a weston.ini must not be read
    w = westonite(no_config_flag=False)
    (w.config_home / "weston.ini").write_text("[core]\n")
    w.wait_ready()
    assert "weston.ini" not in w.log()


def test_home_config_fallback(westonite, tmp_path):
    home = tmp_path / "home"
    (home / ".config").mkdir(parents=True)
    (home / ".config" / "westonite.ini").write_text("[core]\n")
    w = westonite(no_config_flag=False,
                  env={"HOME": str(home), "XDG_CONFIG_HOME": None})
    w.wait_for_log(rf"Using config file '{home}/.config/westonite\.ini'")


def test_no_config_flag_ignores_existing_ini(westonite):
    w = westonite(config="[core]\n", extra_args=["--no-config"])
    w.wait_ready()
    assert "Using config file" not in w.log()
