#!/bin/bash
# Build the Rust shell plugin (R1 hybrid, plan §7) and install it as the
# shipping desktop-shell.so, replacing the meson-installed C shell.
# Runs inside the build container after `ninja install`.
#
# Set WESTONITE_C_ORACLE=1 to skip (keeps the C shell — the behavioral
# oracle, plan R-B).
set -euo pipefail

if [ "${WESTONITE_C_ORACLE:-0}" = "1" ]; then
	echo "WESTONITE_C_ORACLE=1: keeping the C desktop-shell.so"
	exit 0
fi

cd /src
cargo build --locked -p westonite-shell-plugin --release
MODDIR=/usr/lib64/westonite
install -m 755 target/release/libwestonite_shell_plugin.so \
	"$MODDIR/desktop-shell.so"
echo "installed Rust shell -> $MODDIR/desktop-shell.so"
