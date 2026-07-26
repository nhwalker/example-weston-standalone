"""Frontend CLI, socket naming, and config discovery (plan §2.1; the
config interface itself is re-specified at R2a -- plan §5, D9-D12).

Direct-subprocess tests (no fixture) cover invocations that must fail
or exit immediately; fixture tests cover behaviors of a running
compositor.  Tests run against whichever frontend the runner selected
(WESTONITE_BIN + WESTONITE_CONFIG_FORMAT): shared behaviors assert the
same thing in both modes; interface details that changed by design
(clap rejections, TOML discovery, `-o` overrides, the legacy-ini hint)
are mode-gated.
"""

import pytest

import os
import re
import subprocess

from support.compositor import CONFIG_FORMAT, CONFIG_NAME, wait_until

WESTONITE = os.environ.get("WESTONITE_BIN", "westonite")

toml_only = pytest.mark.skipif(
    CONFIG_FORMAT != "toml",
    reason="re-specified config interface (R2a): Rust frontend only")


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
    if CONFIG_FORMAT == "ini":
        assert "unhandled option: --bogus-option" in log.read_text()
    else:
        # clap rejects unknown flags before startup, on stderr
        assert "--bogus-option" in r.stderr


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


# -- config discovery (P2 search order; westonite.toml since R2a) -------


@pytest.mark.installed
def test_config_found_in_xdg_config_home(westonite):
    w = westonite(config="[core]\n")
    w.wait_for_log(rf"Using config file '.*/{re.escape(CONFIG_NAME)}'")


def test_config_explicit_path(westonite, tmp_path):
    cfg = tmp_path / "custom-name.cfg"
    cfg.write_text("[core]\n")
    w = westonite(extra_args=[f"--config={cfg}"], no_config_flag=False)
    w.wait_for_log(rf"Using config file '{cfg}'")


@pytest.mark.installed
def test_stock_weston_ini_is_ignored(westonite):
    # P2: only westonite's own config name is searched; a stock
    # weston.ini must not be read
    w = westonite(no_config_flag=False)
    (w.config_home / "weston.ini").write_text("[core]\n")
    w.wait_ready()
    assert "weston.ini" not in w.log()


def test_home_config_fallback(westonite, tmp_path):
    home = tmp_path / "home"
    (home / ".config").mkdir(parents=True)
    (home / ".config" / CONFIG_NAME).write_text("[core]\n")
    w = westonite(no_config_flag=False,
                  env={"HOME": str(home), "XDG_CONFIG_HOME": None})
    w.wait_for_log(
        rf"Using config file '{home}/.config/{re.escape(CONFIG_NAME)}'")


def test_no_config_flag_ignores_existing_config(westonite):
    w = westonite(config="[core]\n", extra_args=["--no-config"])
    w.wait_ready()
    assert "Using config file" not in w.log()


# -- re-specified interface (R2a: TOML + clap + `-o`) -------------------


@toml_only
def test_legacy_ini_is_hinted_and_ignored(westonite):
    # D11: a westonite.ini where the TOML is expected logs a one-line
    # hint and changes nothing else
    w = westonite(no_config_flag=False,
                  config_home_files={"westonite.ini": "[core]\nbackend=vnc\n"})
    w.wait_for_log(r"found legacy ini config '.*westonite\.ini'")
    assert "Using config file" not in w.log()


@toml_only
def test_dotted_override_reaches_the_config_tree(tmp_path):
    # -o section.key=value patches the tree before validation: prove it
    # lands by overriding to an invalid backend
    log = tmp_path / "log"
    r = run([WESTONITE, f"--log={log}", "--no-config",
             "-o", "core.backend=bogus"],
            env_extra={"XDG_RUNTIME_DIR": str(tmp_path)})
    assert r.returncode != 0
    assert 'unknown backend "bogus"' in log.read_text()


@toml_only
def test_dotted_override_of_a_hex_valued_key(westonite):
    # TOML reads 0x... as an integer, so an unquoted hex override of a
    # string-spelled key must still resolve rather than dying as a type
    # error -- this is the spelling the docs advertise.
    w = westonite(extra_args=["-o", "shell.background-color=0xff336699"])
    w.wait_ready()
    assert "invalid type" not in w.log()


@toml_only
def test_unknown_config_key_is_fatal(tmp_path):
    # D9: deny_unknown_fields -- a typo'd key is a startup error with
    # the offending key named, replacing weston's silent-typo behavior
    cfg = tmp_path / "westonite.toml"
    cfg.write_text('[core]\nbakend = "headless"\n')
    log = tmp_path / "log"
    r = run([WESTONITE, f"--log={log}", f"--config={cfg}",
             "--backend=headless"],
            env_extra={"XDG_RUNTIME_DIR": str(tmp_path)})
    assert r.returncode != 0
    assert "bakend" in log.read_text()


@toml_only
def test_unported_backend_fails_loudly(tmp_path):
    # R2a gate: requesting a not-yet-ported backend is a startup error,
    # never a silent headless fallback
    log = tmp_path / "log"
    r = run([WESTONITE, "--backend=drm", f"--log={log}", "--no-config"],
            env_extra={"XDG_RUNTIME_DIR": str(tmp_path)})
    assert r.returncode != 0
    assert "not yet ported" in log.read_text()
