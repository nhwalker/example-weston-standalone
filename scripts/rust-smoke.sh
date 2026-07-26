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
cargo build --locked --workspace --examples

echo "== rust 2: unit tests (fence fake-C harness D18, shell policy D20)"
cargo test --locked -p weston --features testsupport
cargo test --locked -p westonite-shell

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

echo "ALL RUST SMOKE TESTS PASSED"
