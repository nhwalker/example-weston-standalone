"""Manage westonite compositor instances for e2e tests.

Each instance gets its own XDG_RUNTIME_DIR, wayland socket name, config
file, and log file. Readiness and state checks poll the log with a
deadline -- never bare sleeps.
"""

import os
import pwd
import re
import signal
import socket
import subprocess
import time


# Which config interface the binary under test speaks (R2a re-spec,
# plan §5): "ini" for the C frontend (the oracle), "toml" for the Rust
# frontend (westonite-rs).  Selected by the runner, not per test.
CONFIG_FORMAT = os.environ.get("WESTONITE_CONFIG_FORMAT", "ini")
CONFIG_NAME = "westonite.toml" if CONFIG_FORMAT == "toml" else "westonite.ini"

# Log line proving the shell is up: the C frontend loads a plugin, the
# Rust frontend's statically linked shell logs its marker.
SHELL_READY_PATTERN = (
    r"westonite-shell: Rust shell initialized"
    if CONFIG_FORMAT == "toml"
    else r"Loading module '.*/desktop-shell\.so'")


def ini_to_config(text):
    """Translate the simple `[section]`/`key=value` configs the tests
    are written in into the format under test.  For TOML: repeated
    sections become array-of-tables and values are typed (bool/int
    bare, everything else quoted) -- the same near-diagonal mapping
    documented for users (D11)."""
    if CONFIG_FORMAT == "ini":
        return text
    # ini section name -> TOML array-of-tables name.  The TOML model is
    # kebab-case throughout, so the one ini section spelled with an
    # underscore ([color_characteristics]) is renamed here too.
    array_sections = {"output": "output",
                      "remote-output": "remote-output",
                      "pipewire-output": "pipewire-output",
                      "color_characteristics": "color-characteristics"}
    lines = []
    for line in text.splitlines():
        s = line.strip()
        if s.startswith("[") and s.endswith("]"):
            name = s[1:-1]
            lines.append(f"[[{array_sections[name]}]]"
                         if name in array_sections else s)
        elif "=" in s and not s.startswith("#"):
            key, value = s.split("=", 1)
            lines.append(f"{key.strip()} = {_toml_value(value.strip())}")
        else:
            lines.append(line)
    return "\n".join(lines) + "\n"


def _toml_value(value):
    if value in ("true", "false"):
        return value
    try:
        int(value)
        return value
    except ValueError:
        pass
    # Floats: the colour-characteristics keys are the first typed-float
    # values in the model, and without this they arrive quoted and fail
    # deserialization as "invalid type: string, expected f64".
    # `float()` also accepts "inf"/"nan", which are not TOML floats, so
    # require a digit.
    try:
        float(value)
        if any(c.isdigit() for c in value):
            return value
    except ValueError:
        pass
    escaped = value.replace("\\", "\\\\").replace('"', '\\"')
    return f'"{escaped}"'


# Multiplier applied to every deadline in this module.  The DRM VM
# (docs/drm-testing.md) falls back to TCG emulation on a host without
# /dev/kvm, where the FIRST compositor start takes ~22s -- cold page
# cache plus llvmpipe EGL init; later starts in the same run are 1.3-2s
# -- which blows this module's 10s deadlines outright.  Scaling them
# where the environment is known to be slow is the honest fix;
# loosening them for everyone would hide a real startup regression on
# the fast path.
TIMEOUT_SCALE = float(os.environ.get("WESTONITE_E2E_TIMEOUT_SCALE", "1"))


class Timeout(AssertionError):
    pass


def wait_until(predicate, deadline=10.0, interval=0.1, message="condition"):
    """Poll predicate() until truthy; raise Timeout after deadline seconds
    (times TIMEOUT_SCALE)."""
    deadline *= TIMEOUT_SCALE
    end = time.monotonic() + deadline
    while time.monotonic() < end:
        value = predicate()
        if value:
            return value
        time.sleep(interval)
    raise Timeout(f"timed out after {deadline}s waiting for {message}")


def free_tcp_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


class Westonite:
    """One westonite process under test."""

    def __init__(self, tmp_path, extra_args=(), config=None, backend="headless",
                 width=640, height=480, env=None, socket_name=None,
                 runtime_dir=None, no_config_flag=True, config_home_files=None):
        self.workdir = tmp_path
        self.workdir.mkdir(parents=True, exist_ok=True)
        self.log_path = tmp_path / "westonite.log"
        self.runtime_dir = runtime_dir or tmp_path / "xdg"
        self.runtime_dir.mkdir(mode=0o700, exist_ok=True)
        self.config_home = tmp_path / "config"
        self.config_home.mkdir(exist_ok=True)
        self.backend = backend
        self.vnc_port = None
        self.socket_name = socket_name

        argv = [os.environ.get("WESTONITE_BIN", "westonite"),
                f"--backend={backend}", f"--log={self.log_path}"]
        if backend == "vnc":
            self.vnc_port = free_tcp_port()
            argv += ["--renderer=pixman",
                     f"--port={self.vnc_port}",
                     "--disable-transport-layer-security"]
        elif backend in ("wayland", "x11", "pipewire"):
            # nested/streaming backends default to the GL renderer and
            # there is no GPU in the test container; pixman is what the
            # vnc leg uses for the same reason
            argv += ["--renderer=pixman"]
        elif backend == "drm":
            # The DRM leg runs in a VM whose only device is vkms (see
            # docs/drm-testing.md); there is no keyboard or mouse, and
            # weston refuses to start without one unless told to.  A
            # property of the environment, not of any one test.
            argv += ["--continue-without-input"]
        if backend in ("vnc", "headless", "wayland", "x11") and width is not None:
            argv += [f"--width={width}", f"--height={height}"]
        if socket_name:
            argv += [f"--socket={socket_name}"]

        if config is None:
            # keep tests hermetic against stray ini files unless a test
            # explicitly wants the config-search behavior itself
            if no_config_flag:
                argv += ["--no-config"]
        else:
            config_file = self.config_home / CONFIG_NAME
            config_file.write_text(ini_to_config(config))
        # Extra files that must exist in XDG_CONFIG_HOME at startup
        # (e.g. a legacy ini for the D11 hint test): {name: content}.
        for name, content in (config_home_files or {}).items():
            (self.config_home / name).write_text(content)
        argv += list(extra_args)

        self.env = dict(os.environ)
        self.env.update({
            "XDG_RUNTIME_DIR": str(self.runtime_dir),
            "XDG_CONFIG_HOME": str(self.config_home),
        })
        self.env.pop("WAYLAND_DISPLAY", None)
        for key, value in (env or {}).items():
            if value is None:
                self.env.pop(key, None)
            else:
                self.env[key] = value

        # cwd: files the compositor (or a child it spawns) creates with
        # relative paths -- capture.wcap, screenshots -- land in the
        # test's own tmp dir instead of wherever pytest was started.
        self.proc = subprocess.Popen(argv, env=self.env,
                                     cwd=str(self.workdir),
                                     stdout=subprocess.DEVNULL,
                                     stderr=subprocess.DEVNULL)

    # -- log helpers ----------------------------------------------------

    def log(self):
        try:
            return self.log_path.read_text(errors="replace")
        except FileNotFoundError:
            return ""

    def wait_for_log(self, pattern, deadline=10.0):
        regex = re.compile(pattern)
        return wait_until(lambda: regex.search(self.log()),
                          deadline=deadline,
                          message=f"log line matching {pattern!r}")

    def wait_ready(self):
        """Wait until the compositor is serving clients: the wayland
        socket exists (created after all backends load, so the VNC
        listener is already up too). Deliberately NO probe-connect to
        the VNC port: neatvnc is a single-client server and treats a
        probe as a client -- its teardown can race the real client's
        connect and kill it (seen as instant connection-closed on slow
        CI runners)."""
        wait_until(self._alive_with_socket, message="wayland socket to exist")
        return self

    def _alive_with_socket(self):
        assert self.proc.poll() is None, (
            f"westonite exited {self.proc.poll()} during startup\n"
            f"--- log ---\n{self.log()}")
        return self._sockets() != []

    def _sockets(self):
        if self.socket_name:
            path = self.runtime_dir / self.socket_name
            return [self.socket_name] if path.is_socket() else []
        return sorted(p.name for p in self.runtime_dir.iterdir()
                      if p.name.startswith("wayland-") and p.is_socket())

    @property
    def wayland_display(self):
        socks = self._sockets()
        assert socks, "no wayland socket"
        return socks[0]

    @property
    def x_display(self):
        """The :N display of this instance's Xwayland (requires
        --xwayland in extra_args); waits for it to be advertised."""
        match = self.wait_for_log(r"listening on display (:\d+)")
        return match.group(1)

    def vnc(self):
        """Connect the test's RFB client, authenticating as the user
        running the suite (see scripts/e2e-test.sh for the PAM setup).
        One reconnect on a stalled or server-closed handshake: busy CI
        runners can stall it, and the compositor may still be draining
        an earlier client's teardown."""
        from .vncclient import RfbError, VncClient
        assert self.vnc_port, "instance was not started with backend='vnc'"
        user = os.environ.get("WESTONITE_VNC_USER",
                              pwd.getpwuid(os.getuid()).pw_name)
        password = os.environ["WESTONITE_VNC_PASSWORD"]
        try:
            return VncClient("127.0.0.1", self.vnc_port, user, password)
        except (TimeoutError, OSError, RfbError):
            assert self.proc.poll() is None, (
                f"westonite died during VNC connect\n--- log ---\n{self.log()}")
            time.sleep(2.0)
            return VncClient("127.0.0.1", self.vnc_port, user, password)

    # -- client helpers -------------------------------------------------

    def client_env(self):
        env = dict(self.env)
        env["WAYLAND_DISPLAY"] = self.wayland_display
        return env

    def spawn(self, argv, **popen_kw):
        return subprocess.Popen(argv, env=self.client_env(), **popen_kw)

    def run_client(self, argv, timeout=15, check=True):
        """Run a Wayland client to completion; return CompletedProcess
        with captured text output. Asserts exit code 0 unless
        check=False (for clients that are EXPECTED to fail)."""
        timeout *= TIMEOUT_SCALE
        result = subprocess.run(argv, env=self.client_env(), timeout=timeout,
                                capture_output=True, text=True)
        assert not check or result.returncode == 0, (
            f"{argv} exited {result.returncode}\n"
            f"stdout:\n{result.stdout}\nstderr:\n{result.stderr}")
        return result

    # -- shutdown -------------------------------------------------------

    def wait_exit(self, deadline=15.0):
        """Wait for the compositor to exit on its own; return exit code."""
        deadline *= TIMEOUT_SCALE
        try:
            return self.proc.wait(deadline)
        except subprocess.TimeoutExpired:
            raise Timeout(
                f"westonite still running after {deadline}s\n"
                f"--- log ---\n{self.log()}")

    def terminate(self, sig=signal.SIGTERM, deadline=10.0):
        """Signal the compositor and assert it exits cleanly (code 0)."""
        deadline *= TIMEOUT_SCALE
        if self.proc.poll() is None:
            self.proc.send_signal(sig)
        try:
            code = self.proc.wait(deadline)
        except subprocess.TimeoutExpired:
            self.proc.kill()
            raise AssertionError(
                f"westonite did not exit within {deadline}s of {sig!r}\n"
                f"--- log ---\n{self.log()}")
        assert code == 0, (
            f"westonite exited {code}\n--- log ---\n{self.log()}")
        return code

    def kill(self):
        if self.proc.poll() is None:
            self.proc.kill()
            self.proc.wait(5)
