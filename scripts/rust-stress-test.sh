#!/bin/bash
# Destroy-storm stress test (R1 exit criterion, plan §7/D18): client
# add/kill churn against the hybrid build (C frontend + Rust shell),
# run under valgrind memcheck — the invalidation paths §3b/§3f
# introduce are exactly what this exercises.  (Rust ASAN is nightly-only
# and cannot instrument the C half of the hybrid; valgrind memchecks
# the whole process instead.  The pure-Rust ASAN leg lives in
# rust-asan-smoke.sh.)
# Runs inside the build container after the meson+rust install
# (smoke-test.sh or: ninja install && rust-shell-install.sh, plus
# -De2e-test-client=true for wtest-client).
set -euo pipefail

cd /src
WTEST=${WTEST_CLIENT:-/src/build/tests/e2e/clients/wtest-client}
[ -x "$WTEST" ] || { echo "wtest-client missing (build with -De2e-test-client=true)"; exit 1; }

export XDG_RUNTIME_DIR=/tmp/xdg-stress
mkdir -p -m 0700 "$XDG_RUNTIME_DIR"
SOCKET=stress-0
LOG=/tmp/stress-westonite.log
VG=/tmp/stress-valgrind.log
CLIENTS=/tmp/stress-clients.log
: > "$CLIENTS"

valgrind --error-exitcode=42 --leak-check=full \
	--errors-for-leak-kinds=definite --log-file="$VG" \
	westonite --backend=headless --socket="$SOCKET" --log="$LOG" &
WPID=$!

for i in $(seq 1 40); do
	if [ -S "$XDG_RUNTIME_DIR/$SOCKET" ]; then break; fi
	sleep 0.5
done
[ -S "$XDG_RUNTIME_DIR/$SOCKET" ] || { echo "compositor socket never appeared"; cat "$LOG"; exit 1; }

export WAYLAND_DISPLAY="$SOCKET"

echo "== storm 1: sequential add/kill churn (surface add/remove + focus hunt)"
for i in $(seq 1 25); do
	"$WTEST" --size 200x100 --title "churn-$i" >>"$CLIENTS" 2>&1 &
	CPID=$!
	sleep 0.15
	kill -9 "$CPID" 2>/dev/null || true
	wait "$CPID" 2>/dev/null || true
done

echo "== storm 2: concurrent clients, staggered kills (focus churn)"
PIDS=()
for i in $(seq 1 6); do
	"$WTEST" --size 160x120 --title "pack-$i" >>"$CLIENTS" 2>&1 &
	PIDS+=($!)
	sleep 0.1
done
sleep 0.5
# Kill in non-creation order: middle, first, last, rest.
for idx in 3 0 5 1 4 2; do
	kill -9 "${PIDS[$idx]}" 2>/dev/null || true
done
for p in "${PIDS[@]}"; do wait "$p" 2>/dev/null || true; done
sleep 0.5

# NOTE: wtest-client has no transient/child mode; all three storms churn
# single xdg toplevels.  Transient (parent/child) teardown is covered by
# the e2e suite only.
echo "== storm 3: rapid short-lived toplevel churn (map/kill singles)"
for i in $(seq 1 10); do
	"$WTEST" --size 200x100 --title "single-$i" >>"$CLIENTS" 2>&1 &
	CPID3=$!
	sleep 0.15
	kill -9 "$CPID3" 2>/dev/null || true
	wait "$CPID3" 2>/dev/null || true
done

# The storms must have exercised something real: wtest-client prints
# "mapped: WxH" once its first configure is acked.  Zero mapped clients
# means the whole run tested nothing (shell rejecting surfaces, client
# protocol error) — fail instead of reporting a hollow valgrind-clean.
MAPPED=$(grep -c '^mapped:' "$CLIENTS" || true)
if [ "${MAPPED:-0}" -eq 0 ]; then
	echo "FAIL: no wtest-client ever mapped a surface (storms exercised nothing)"
	cat "$CLIENTS"
	kill -9 "$WPID" 2>/dev/null || true
	wait "$WPID" 2>/dev/null || true
	tail -20 "$LOG"
	exit 1
fi
echo "clients mapped during storms: $MAPPED"

kill -TERM "$WPID"
# Teardown deadline: a hung teardown must fail in minutes, not at the CI
# job timeout.  120s is generous even under valgrind.  Poll-then-reap
# instead of a background watchdog subshell: nothing lingers past the
# script and no stale-PID kill can fire after exit.
for _ in $(seq 1 240); do
	kill -0 "$WPID" 2>/dev/null || break
	sleep 0.5
done
if kill -0 "$WPID" 2>/dev/null; then
	kill -9 "$WPID" 2>/dev/null || true
	wait "$WPID" 2>/dev/null || true
	echo "FAIL: compositor teardown hung >120s"; tail -40 "$VG"; tail -20 "$LOG"; exit 1
fi
wait "$WPID" || { echo "FAIL: compositor exited non-zero (valgrind errors or crash)"; tail -40 "$VG"; tail -20 "$LOG"; exit 1; }
grep -q "ERROR SUMMARY: 0 errors" "$VG" || { echo "FAIL: valgrind errors"; tail -40 "$VG"; exit 1; }

echo "DESTROY-STORM STRESS PASSED (valgrind clean, $MAPPED clients mapped)"
