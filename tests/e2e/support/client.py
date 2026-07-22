"""Drive wtest-client instances (tests/e2e/clients/wtest-client.c).

The client prints one event per line on stdout; a reader thread collects
them so tests can wait for protocol facts (mapped, focus changes,
wm_capabilities, configure states) with deadlines.
"""

import os
import re
import signal
import subprocess
import threading

from .compositor import wait_until


class WtestClient:
    def __init__(self, compositor, *args):
        binary = os.environ.get("WTEST_CLIENT", "wtest-client")
        self.proc = compositor.spawn([binary, *args],
                                     stdout=subprocess.PIPE,
                                     stderr=subprocess.STDOUT, text=True)
        self.lines = []
        self._lock = threading.Lock()
        self._reader = threading.Thread(target=self._read, daemon=True)
        self._reader.start()

    def _read(self):
        for line in self.proc.stdout:
            with self._lock:
                self.lines.append(line.rstrip("\n"))

    def output(self):
        with self._lock:
            return list(self.lines)

    def count(self, pattern):
        regex = re.compile(pattern)
        return sum(1 for line in self.output() if regex.search(line))

    def wait_for_count(self, pattern, n, deadline=10.0):
        wait_until(lambda: self.count(pattern) >= n, deadline=deadline,
                   message=f"{n}th client line matching {pattern!r}")

    def wait_for_line(self, pattern, deadline=10.0):
        regex = re.compile(pattern)

        def find():
            for line in self.output():
                match = regex.search(line)
                if match:
                    return match
            return None

        return wait_until(find, deadline=deadline,
                          message=f"client line matching {pattern!r}")

    def wait_mapped(self):
        match = self.wait_for_line(r"mapped: (\d+)x(\d+)")
        return int(match.group(1)), int(match.group(2))

    def pause(self):
        self.proc.send_signal(signal.SIGUSR1)
        self.wait_for_line(r"^paused$")

    def resume(self):
        self.proc.send_signal(signal.SIGUSR2)
        self.wait_for_line(r"^resumed$")

    def terminate(self):
        if self.proc.poll() is None:
            self.proc.terminate()
            try:
                self.proc.wait(5)
            except subprocess.TimeoutExpired:
                self.proc.kill()

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.terminate()
