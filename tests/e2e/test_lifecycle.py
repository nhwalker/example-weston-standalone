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


def test_clean_shutdown_vnc_backend(westonite):
    w = westonite(backend="vnc")
    w.terminate(signal.SIGTERM)
