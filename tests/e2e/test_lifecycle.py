"""Compositor lifecycle: clean startup and shutdown (plan §2.1)."""

import signal

import pytest


@pytest.mark.installed
def test_clean_shutdown_sigterm(westonite):
    w = westonite()
    w.terminate(signal.SIGTERM)


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
