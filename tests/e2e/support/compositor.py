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


class Timeout(AssertionError):
    pass


def wait_until(predicate, deadline=10.0, interval=0.1, message="condition"):
    """Poll predicate() until truthy; raise Timeout after deadline seconds."""
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
                 runtime_dir=None, no_config_flag=True):
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
        if backend in ("vnc", "headless") and width is not None:
            argv += [f"--width={width}", f"--height={height}"]
        if socket_name:
            argv += [f"--socket={socket_name}"]

        if config is None:
            # keep tests hermetic against stray ini files unless a test
            # explicitly wants the config-search behavior itself
            if no_config_flag:
                argv += ["--no-config"]
        else:
            config_file = self.config_home / "westonite.ini"
            config_file.write_text(config)
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

        self.proc = subprocess.Popen(argv, env=self.env,
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
        socket exists (created after backend + shell load succeed) and,
        for the VNC backend, the RFB port accepts connections."""
        wait_until(self._alive_with_socket, message="wayland socket to exist")
        if self.backend == "vnc":
            wait_until(self._vnc_port_open, message="VNC port to accept")
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

    def _vnc_port_open(self):
        try:
            with socket.create_connection(("127.0.0.1", self.vnc_port), 0.5):
                return True
        except OSError:
            return False

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
        One reconnect on timeout: busy CI runners can stall the very
        first handshake."""
        from .vncclient import VncClient
        assert self.vnc_port, "instance was not started with backend='vnc'"
        user = os.environ.get("WESTONITE_VNC_USER",
                              pwd.getpwuid(os.getuid()).pw_name)
        password = os.environ["WESTONITE_VNC_PASSWORD"]
        try:
            return VncClient("127.0.0.1", self.vnc_port, user, password)
        except (TimeoutError, OSError):
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

    def run_client(self, argv, timeout=15):
        """Run a Wayland client to completion; return CompletedProcess
        with captured text output. Asserts exit code 0."""
        result = subprocess.run(argv, env=self.client_env(), timeout=timeout,
                                capture_output=True, text=True)
        assert result.returncode == 0, (
            f"{argv} exited {result.returncode}\n"
            f"stdout:\n{result.stdout}\nstderr:\n{result.stderr}")
        return result

    # -- shutdown -------------------------------------------------------

    def wait_exit(self, deadline=15.0):
        """Wait for the compositor to exit on its own; return exit code."""
        try:
            return self.proc.wait(deadline)
        except subprocess.TimeoutExpired:
            raise Timeout(
                f"westonite still running after {deadline}s\n"
                f"--- log ---\n{self.log()}")

    def terminate(self, sig=signal.SIGTERM, deadline=10.0):
        """Signal the compositor and assert it exits cleanly (code 0)."""
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
