#!/bin/bash
# Rust-migration R0 smoke (plan §7 R0 exit criterion): build the cargo
# workspace, run the fence-crate unit tests, then bring up the r0-smoke
# binary (compositor + headless backend + noop renderer), SIGTERM it,
# and require a clean exit — plus a valgrind pass over the same run.
# Runs inside the containers/Containerfile.build image.
set -euo pipefail

cd /src

export XDG_RUNTIME_DIR=/tmp/xdg
mkdir -p -m 0700 "$XDG_RUNTIME_DIR"

fail() { echo "FAIL: $1" >&2; exit 1; }

# Poll a log file for a marker instead of sleeping a fixed time: fast
# when startup is fast, and a real deadline (not a race) when it isn't.
wait_for_marker() { # <file> <marker> <deadline-halfseconds>
	local f=$1 marker=$2 tries=$3
	for _ in $(seq 1 "$tries"); do
		grep -q "$marker" "$f" 2>/dev/null && return 0
		sleep 0.5
	done
	return 1
}

echo "== rust 1: workspace builds"
# --examples alone selects ONLY example targets; --bins must be listed
# too or the westonite-rs binary the frontend legs run never builds.
cargo build --locked --workspace --examples --bins

echo "== rust 2: unit tests (fence fake-C harness D18, shell policy D20, config/spawn R2a)"
cargo test --locked -p weston --features testsupport
cargo test --locked -p westonite-shell
cargo test --locked -p westonite-config
cargo test --locked -p westonite-spawn

echo "== rust 3: r0-smoke runs headless and exits 0 on SIGTERM"
target/debug/examples/r0-smoke > /tmp/r0.log 2>&1 &
PID=$!
wait_for_marker /tmp/r0.log "Output 'headless' enabled" 20 \
	|| { cat /tmp/r0.log >&2; fail "headless output was not enabled"; }
kill -0 "$PID" || { cat /tmp/r0.log >&2; fail "r0-smoke died during startup"; }
kill -TERM "$PID"
wait "$PID" || { cat /tmp/r0.log >&2; fail "r0-smoke exited non-zero on SIGTERM"; }
grep -q "westonite-r0: clean exit (0)" /tmp/r0.log \
	|| { cat /tmp/r0.log >&2; fail "no clean-exit marker"; }

echo "== rust 4: same run under valgrind (0 errors, 0 definite leaks)"
valgrind --error-exitcode=42 --leak-check=full \
	--errors-for-leak-kinds=definite \
	target/debug/examples/r0-smoke > /tmp/r0-vg.log 2>&1 &
VPID=$!
# Same assertions as the plain leg: the valgrind pass must demonstrably
# reach the enabled-output state and exit through the clean path, or it
# has valgrinded nothing.
wait_for_marker /tmp/r0-vg.log "Output 'headless' enabled" 120 \
	|| { cat /tmp/r0-vg.log >&2; fail "valgrind leg: headless output was not enabled"; }
kill -0 "$VPID" || { cat /tmp/r0-vg.log >&2; fail "valgrind leg: died during startup"; }
kill -TERM "$VPID"
wait "$VPID" || { cat /tmp/r0-vg.log >&2; fail "valgrind reported errors or leaks"; }
grep -q "westonite-r0: clean exit (0)" /tmp/r0-vg.log \
	|| { cat /tmp/r0-vg.log >&2; fail "valgrind leg: no clean-exit marker"; }

echo "== rust 5: westonite-rs frontend headless (R2a) exits 0 on SIGTERM"
# Fresh log every run: weston opens --log in append mode, so a stale
# file from an earlier container run would satisfy the marker greps.
rm -f /tmp/rs.log /tmp/rs-vg.log
target/debug/westonite-rs --backend=headless --no-config --log=/tmp/rs.log &
PID=$!
wait_for_marker /tmp/rs.log "westonite: wayland socket" 20 \
	|| { cat /tmp/rs.log >&2; fail "frontend: no wayland socket"; }
grep -q "westonite-shell: Rust shell initialized" /tmp/rs.log \
	|| { cat /tmp/rs.log >&2; fail "frontend: shell marker missing"; }
kill -TERM "$PID"
wait "$PID" || { cat /tmp/rs.log >&2; fail "frontend exited non-zero on SIGTERM"; }

echo "== rust 6: frontend + autolaunch watch under valgrind (SIGCHLD path)"
printf '#!/bin/sh\nsleep 2\n' > /tmp/rs-stub && chmod +x /tmp/rs-stub
valgrind --error-exitcode=42 --leak-check=full \
	--errors-for-leak-kinds=definite \
	target/debug/westonite-rs --backend=headless --no-config \
	--log=/tmp/rs-vg.log -o autolaunch.path=/tmp/rs-stub \
	-o autolaunch.watch=true \
	|| { cat /tmp/rs-vg.log >&2; fail "frontend valgrind reported errors or leaks"; }
grep -q "autolaunched client exited, terminating" /tmp/rs-vg.log \
	|| { cat /tmp/rs-vg.log >&2; fail "frontend valgrind leg: watch exit not exercised"; }

echo "== rust 7: westonite-rs VNC backend (R2c) under valgrind"
# No VNC client here (that needs the PAM stack — the e2e leg covers
# it); this gates startup/shutdown of the VNC path.  Suppressions
# cover the verified upstream vnc-backend leaks only (see the .supp).
rm -f /tmp/rs-vnc.log
valgrind --error-exitcode=42 --leak-check=full \
	--errors-for-leak-kinds=definite \
	--suppressions=/src/scripts/valgrind-upstream-vnc.supp \
	target/debug/westonite-rs --backend=vnc --renderer=pixman \
	--port=59920 --disable-transport-layer-security --no-config \
	--log=/tmp/rs-vnc.log > /tmp/rs-vnc-vg.log 2>&1 &
VNCPID=$!
wait_for_marker /tmp/rs-vnc.log "westonite: wayland socket" 120 \
	|| { cat /tmp/rs-vnc.log /tmp/rs-vnc-vg.log >&2; fail "vnc frontend: no wayland socket"; }
# The socket is bound BEFORE the heads flush, so it proves only that the
# backend loaded.  Wait for the output too, or the VNC configure path
# (vnc_output_set_size / resizeable / forced-normal transform) is not
# actually under valgrind here.
wait_for_marker /tmp/rs-vnc.log "Output 'vnc' enabled" 120 \
	|| { cat /tmp/rs-vnc.log /tmp/rs-vnc-vg.log >&2; fail "vnc frontend: output not enabled"; }
grep -q "westonite-shell: Rust shell initialized" /tmp/rs-vnc.log \
	|| { cat /tmp/rs-vnc.log >&2; fail "vnc frontend: shell marker missing"; }
kill -TERM "$VNCPID"
wait "$VNCPID" || { cat /tmp/rs-vnc-vg.log >&2; fail "vnc frontend valgrind reported errors or leaks"; }

echo "ALL RUST SMOKE TESTS PASSED"
