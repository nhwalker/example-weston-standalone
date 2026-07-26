#!/bin/bash
# Build westonite from /src and run the Phase 1-3 smoke tests.
# Runs inside the containers/Containerfile.build image.
set -euo pipefail

cd /src
rm -rf build
meson setup build --prefix=/usr
ninja -C build
ninja -C build install

# R1+: the shipping shell is the Rust plugin (docs/rust-migration-plan.md
# §7); WESTONITE_C_ORACLE=1 keeps the C shell as the behavioral oracle.
/src/scripts/rust-shell-install.sh

export XDG_RUNTIME_DIR=/tmp/xdg
mkdir -p -m 0700 "$XDG_RUNTIME_DIR"
mkdir -p -m 1777 /tmp/.X11-unix

fail() { echo "FAIL: $1" >&2; [ -f "$2" ] && cat "$2" >&2; exit 1; }

echo "== smoke 1: headless, no config, no helper clients"
timeout --preserve-status 5 westonite --backend=headless --log=/tmp/w1.log \
	|| fail "westonite exited non-zero" /tmp/w1.log
grep -q "Loading module '/usr/lib64/westonite/desktop-shell.so'" /tmp/w1.log \
	|| fail "desktop-shell.so not loaded" /tmp/w1.log
# The module path is identical for both shells, so assert which one
# actually ran: the Rust shell logs an identifying marker at init.
if [ "${WESTONITE_C_ORACLE:-0}" = "1" ]; then
	grep -q "westonite-shell: Rust shell initialized" /tmp/w1.log \
		&& fail "Rust shell ran in C-oracle mode" /tmp/w1.log
else
	grep -q "westonite-shell: Rust shell initialized" /tmp/w1.log \
		|| fail "Rust shell marker missing (C shell silently shipped?)" /tmp/w1.log
fi

echo "== smoke 2: westonite.ini is honored"
export XDG_CONFIG_HOME=/tmp/cfg
mkdir -p "$XDG_CONFIG_HOME"
printf '[shell]\nbackground-color=0xff336699\n' > "$XDG_CONFIG_HOME/westonite.ini"
timeout --preserve-status 5 westonite --backend=headless --log=/tmp/w2.log \
	|| fail "westonite exited non-zero" /tmp/w2.log
grep -q "Using config file '/tmp/cfg/westonite.ini'" /tmp/w2.log \
	|| fail "westonite.ini not picked up" /tmp/w2.log

echo "== smoke 3: Xwayland round-trip"
westonite --backend=headless --xwayland --log=/tmp/w3.log &
WPID=$!
sleep 2
DISP=$(grep -oP 'listening on display \K:[0-9]+' /tmp/w3.log) \
	|| fail "no X display advertised" /tmp/w3.log
DISPLAY=$DISP xdpyinfo > /tmp/xdpy.out 2>&1 \
	|| fail "xdpyinfo could not query Xwayland" /tmp/xdpy.out
kill $WPID
wait $WPID || fail "westonite exited non-zero after Xwayland test" /tmp/w3.log
grep -q "launching '/usr/bin/Xwayland'" /tmp/w3.log \
	|| fail "Xwayland was not spawned" /tmp/w3.log

echo "ALL SMOKE TESTS PASSED"
