"""Compositor lifecycle: clean startup and shutdown (plan §2.1)."""

import signal

import pytest

from support.compositor import CONFIG_FORMAT

# The socket-marker line asserted below is a Rust-frontend log line;
# the C oracle proves the same ordering by construction (main.c:4706).
toml_only = pytest.mark.skipif(
    CONFIG_FORMAT != "toml",
    reason="asserts on a Rust-frontend log marker")


@pytest.mark.installed
def test_clean_shutdown_sigterm(westonite):
    w = westonite()
    w.terminate(signal.SIGTERM)


@toml_only
def test_socket_binds_after_outputs_are_configured(westonite):
    """C binds the wayland socket only after the heads flush and its
    init_failed check (main.c:4671 -> 4706), so the socket's on-disk
    existence implies the outputs are configured -- the exact inference
    this harness's wait_ready and the smoke scripts draw.  PR16-C6 had
    the port binding the socket before the flush, which made that
    inference silently false."""
    w = westonite()
    log = w.log()
    i_output = log.find("' enabled")
    i_socket = log.find("westonite: wayland socket")
    assert i_output != -1 and i_socket != -1, log
    assert i_output < i_socket, log


def test_clean_shutdown_sigint(westonite):
    w = westonite()
    w.terminate(signal.SIGINT)


def test_clean_shutdown_sigusr2(westonite):
    """SIGUSR2 is a first-class termination signal in C -- it shares
    on_term_signal with SIGTERM (main.c:4546)."""
    w = westonite()
    w.terminate(signal.SIGUSR2)


def test_sigterm_logs_caught_signal(westonite):
    """C's on_term_signal logs the number before terminating
    (main.c:831)."""
    w = westonite()
    w.terminate(signal.SIGTERM)
    assert f"caught signal {int(signal.SIGTERM)}" in w.log()


def test_sigint_reroutes_through_sigusr2(westonite):
    """C catches SIGINT with plain sigaction -- NOT the event loop's
    signalfd, so gdb can still trap Ctrl+C -- and the handler
    re-raises SIGUSR2, which is on the loop (main.c:4546-4567).  The
    observable proxy for that mechanism: the shutdown log names
    SIGUSR2's number, never SIGINT's."""
    w = westonite()
    w.terminate(signal.SIGINT)
    log = w.log()
    assert f"caught signal {int(signal.SIGUSR2)}" in log
    assert f"caught signal {int(signal.SIGINT)}" not in log


def test_clean_shutdown_vnc_backend(westonite):
    w = westonite(backend="vnc")
    w.terminate(signal.SIGTERM)
