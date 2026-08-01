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
import pathlib
import re
import signal
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


def test_option_for_an_unloaded_backend_is_fatal(tmp_path):
    # C hands the CLI to each loaded backend's parse_options in turn and
    # then treats whatever is left in argv as `fatal: unhandled option`.
    # --seat is a DRM option, so a headless run must refuse it instead
    # of accepting it as a no-op.
    log = tmp_path / "log"
    r = run([WESTONITE, "--backend=headless", "--seat=seat1",
             f"--log={log}", "--no-config"],
            env_extra={"XDG_RUNTIME_DIR": str(tmp_path)})
    assert r.returncode != 0
    assert "unhandled option: --seat" in log.read_text()


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
def test_unsupported_feature_fails_loudly(tmp_path):
    """R2a gate: a requested-but-unavailable feature is a startup
    error, never a silent no-op.

    This has moved three times as the migration ate its targets --
    `--backend=drm`, then `[libinput]` as a whole, both ported now.  It
    points at `touchscreen-calibrator`, which is no longer *unported*
    but *unsupported*: the weston_touch_calibration protocol is a
    product decision to drop (2026-08-01), since westonite ships no
    calibrator client to drive it.

    The gate is the principle, not any one feature: something the user
    asked for and will not get has to say so.
    """
    cfg = tmp_path / "westonite.toml"
    cfg.write_text("[libinput]\ntouchscreen-calibrator = true\n")
    log = tmp_path / "log"
    r = run([WESTONITE, "--backend=headless", f"--log={log}", f"--config={cfg}"],
            env_extra={"XDG_RUNTIME_DIR": str(tmp_path)})
    assert r.returncode != 0
    text = log.read_text()
    assert "not supported by westonite" in text, text
    # Not a promise of a future slice: there is no slice coming.
    assert "not yet ported" not in text, text


@toml_only
def test_libinput_device_settings_without_drm_are_inert(westonite):
    """The per-device settings reach libinput only through the DRM
    backend's configure_device hook, so without DRM they do nothing --
    in C too.  Refusing them here would be inventing an error C has
    not got, so the section must simply be accepted and ignored.

    The fixture asserts it for us: it waits for the socket (so startup
    succeeded) and requires exit 0 at teardown."""
    w = westonite(config="[libinput]\nenable-tap=true\n")
    assert "fatal" not in w.log(), w.log()


@toml_only
@pytest.mark.parametrize("section,expected", [
    ('accel-profile = "swift"', "not a valid accel-profile"),
    ('accel-speed = 4.0', "out of range"),
    ('scroll-method = "twofinger"', "not a valid scroll-method"),
    ('scroll-method = "button"\nscroll-button = "BTN_NONSENSE"',
     "not an evdev button name"),
    ('scroll-button = "BTN_RIGHT"', 'only applies with scroll-method'),
])
def test_libinput_bad_values_are_startup_errors(tmp_path, section, expected):
    """Where C warns per device and leaves the key inert, the Rust
    frontend refuses at startup -- a setting that was asked for and
    does nothing is the failure mode this migration exists to remove.

    Unconditional, not gated on DRM: these values could not work on any
    backend, so rejecting them cannot mask a config that would have
    run.  (The *valid* keys stay inert without DRM -- the test above.)
    """
    cfg = tmp_path / "westonite.toml"
    cfg.write_text(f"[libinput]\n{section}\n")
    log = tmp_path / "log"
    r = run([WESTONITE, "--backend=headless", f"--log={log}", f"--config={cfg}"],
            env_extra={"XDG_RUNTIME_DIR": str(tmp_path)})
    assert r.returncode != 0
    assert expected in log.read_text(), log.read_text()


# ---------------------------------------------------------------------
# Logging: scopes, subscribers, the flight recorder (R2f).
#
# Shared with the C oracle -- these are libweston's own machinery and
# both frontends wire it the same way, which is the point of the tests.
# ---------------------------------------------------------------------

TIMESTAMP = r"^\[\d\d:\d\d:\d\d\.\d\d\d\] "


def test_log_lines_are_timestamped(westonite):
    """Every line goes through the "log" scope, and the scope handler
    timestamps it.  Was not true of the Rust frontend before R2f: it
    wrote to the file directly and produced bare lines, so this is the
    parity check that the port did not merely add a prefix."""
    w = westonite()
    w.wait_for_log(r"Command line:")
    stamped = [ln for ln in w.log().splitlines() if re.match(TIMESTAMP, ln)]
    assert stamped, w.log()
    # Continuation lines (weston_log_continue) deliberately have none --
    # that is how a multi-line block stays visually attached.
    assert any(ln.startswith(" ") for ln in w.log().splitlines()), w.log()


def test_flight_recorder_is_on_by_default(westonite):
    """C creates the recorder from a default scope list whenever
    --flight-rec-scopes is absent (main.c:4515) and says so at
    startup."""
    w = westonite()
    w.wait_for_log(r"Flight recorder: enabled")


def test_empty_flight_rec_scopes_disables_the_recorder(westonite):
    """The one way to turn it off: an explicitly empty list.  Absent and
    empty are different values, which is why this is worth pinning."""
    w = westonite(extra_args=["--flight-rec-scopes="])
    w.wait_for_log(r"Flight recorder: disabled")


def test_logger_scopes_redirect_the_log_file(westonite):
    """--logger-scopes replaces the logger's default "log" subscription
    rather than adding to it, so pointing it elsewhere empties the log
    file of ordinary output.  The compositor still starts -- the
    fixture waits on the *socket*, not on a log line, which is what
    makes this testable at all."""
    w = westonite(extra_args=["--logger-scopes=drm-backend"])
    assert "Command line:" not in w.log(), w.log()


def test_logger_scopes_log_keeps_the_default(westonite):
    """The other half: naming "log" explicitly is the default spelled
    out, so ordinary output survives.  Without this the test above
    would also pass if the flag simply broke logging."""
    w = westonite(extra_args=["--logger-scopes=log"])
    w.wait_for_log(r"Command line:")


def test_wait_for_debugger_stops_the_process(westonite):
    """C raises SIGSTOP on itself after loading the config and before
    creating anything (main.c:4589).  Assert the real thing: the
    process is in state T, and SIGCONT gets it running."""
    w = westonite(extra_args=["--wait-for-debugger"], wait=False)
    w.wait_for_log(r"waiting for debugger, send SIGCONT to continue")

    def stopped():
        stat = pathlib.Path(f"/proc/{w.proc.pid}/stat").read_text()
        # The comm field can contain spaces/parens; state is the field
        # right after the closing paren.
        return stat.rsplit(")", 1)[1].split()[0] == "T"

    wait_until(stopped, message="the compositor to stop for the debugger")
    # Nothing exists yet: the socket comes long after this point.
    assert w._sockets() == []
    os.kill(w.proc.pid, signal.SIGCONT)
    w.wait_ready()


@toml_only
def test_modules_are_refused_by_design(tmp_path):
    """Third-party plugin loading is dropped by product decision
    (2026-08-01), reversing plan D2.  Nothing westonite ships uses it:
    the shell is linked in, and both plugins that loaded this way
    (remoting, pipewire-output) are themselves dropped.

    The key stays in the config model on purpose -- so this refusal can
    say what happened, instead of the generic unknown-field error a
    removed field would produce."""
    cfg = tmp_path / "westonite.toml"
    cfg.write_text('[core]\nmodules = ["something.so"]\n')
    log = tmp_path / "log"
    r = run([WESTONITE, "--backend=headless", f"--log={log}", f"--config={cfg}"],
            env_extra={"XDG_RUNTIME_DIR": str(tmp_path)})
    assert r.returncode != 0
    text = log.read_text()
    assert "not supported by westonite" in text, text
    # Specifically NOT the "use the C westonite" promise of a future
    # slice: there is no slice coming.
    assert "not yet ported" not in text, text


@toml_only
def test_modules_cli_flag_is_refused_too(tmp_path):
    """Same decision, reached through --modules rather than the file."""
    log = tmp_path / "log"
    r = run([WESTONITE, "--backend=headless", f"--log={log}", "--no-config",
             "--modules=something.so"],
            env_extra={"XDG_RUNTIME_DIR": str(tmp_path)})
    assert r.returncode != 0
    assert "not supported by westonite" in log.read_text()


# ---------------------------------------------------------------------
# --debug, the protocol dump, and the two [core] renderer aliases.
# Shared with the C oracle: libweston machinery, wired the same way.
# ---------------------------------------------------------------------

def test_protocol_dump_scope(westonite):
    """The `proto` scope and its wl_protocol_logger are installed
    unconditionally (main.c:4618) -- `--debug` does not gate them.
    They cost nothing until someone subscribes, which is what
    --logger-scopes=proto does."""
    w = westonite(extra_args=["--logger-scopes=proto"], socket_name="proto-probe")
    w.run_client(["wayland-info"])
    log = w.log()
    assert re.search(r"rq wl_display@1\.get_registry\(new id wl_registry@2\)", log), log
    # Argument decoding, not just the message name: a string, two ints
    # and a generic new-id in one line.
    assert re.search(r'rq wl_registry@2\.bind\(\d+, "wl_\w+", \d+, new id \[unknown\]@\d+\)',
                     log), log


def test_protocol_dump_is_silent_without_a_subscriber(westonite):
    """The other half: no subscription, no dump.  Without this, the
    test above would also pass if we logged unconditionally -- which
    would put every client's traffic in every user's log file."""
    w = westonite()
    w.run_client(["wayland-info"])
    assert "wl_display@1.get_registry" not in w.log(), w.log()


def test_debug_protocol_advertises_the_global(westonite):
    """--debug enables libweston's weston-debug protocol, which is
    observable where it matters: the global appears in the registry."""
    w = westonite(extra_args=["--debug"])
    assert "weston_debug_v1" in w.run_client(["wayland-info"]).stdout


def test_debug_protocol_is_off_by_default(westonite):
    """It stays off unless asked: the global is a debugging interface
    and the flag also makes every client able to screenshot every
    output, so a default-on would be a privilege leak."""
    w = westonite()
    assert "weston_debug_v1" not in w.run_client(["wayland-info"]).stdout


def test_output_decorations_reach_the_backend(westonite):
    """`[core] output-decorations` is headless-only in C too (a field of
    weston_headless_backend_config).  Asserted through its failure
    mode rather than its pixels: decorations need the GL or Vulkan
    renderer, and the headless backend refuses them on noop/pixman
    (headless.c:731).  A key that never reached the backend would let
    startup succeed, so this is the plumbing check."""
    w = westonite(config="[core]\noutput-decorations=true\n", wait=False)
    assert w.wait_exit() != 0, w.log()
    assert "decorations" in w.log(), w.log()


def test_use_pixman_config_key_conflicts_with_renderer(westonite):
    """`[core] use-pixman` / `use-gl` are deprecated aliases for
    `renderer=`, and C rejects the combination (main.c:3511).  The
    conflict is the cheapest proof the *file* spelling is read at all
    -- before this slice only the CLI flags were."""
    w = westonite(config='[core]\nuse-pixman=true\nrenderer=gl\n', wait=False)
    assert w.wait_exit() != 0, w.log()
    assert "Conflicting renderer specifications" in w.log(), w.log()
