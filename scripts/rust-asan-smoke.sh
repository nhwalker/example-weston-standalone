#!/bin/bash
# ASAN leg of the R0 exit criterion (plan §7 R0, D18): build the
# r0-smoke binary with AddressSanitizer and require the same clean
# SIGTERM exit.  Rust sanitizers are nightly-only, so this leg uses a
# rustup nightly toolchain (build/CI validation tool only — shipped
# binaries stay on the EL10 rust-toolset per plan §6).
# Runs inside the containers/Containerfile.build image; needs network
# on first run (rustup + crates).
set -euo pipefail

cd /src

export RUSTUP_HOME="${RUSTUP_HOME:-/tmp/rustup}"
export CARGO_HOME="${ASAN_CARGO_HOME:-${CARGO_HOME:-/tmp/asan-cargo}}"
if ! command -v rustup >/dev/null 2>&1 && [ ! -x "$CARGO_HOME/bin/rustup" ]; then
	curl -sSf https://sh.rustup.rs | sh -s -- -y \
		--default-toolchain nightly --profile minimal --component rust-src
fi
export PATH="$CARGO_HOME/bin:$PATH"
rustup toolchain list | grep -q nightly || \
	rustup toolchain install nightly --profile minimal --component rust-src

export XDG_RUNTIME_DIR=/tmp/xdg
mkdir -p -m 0700 "$XDG_RUNTIME_DIR"

echo "== asan: build r0-smoke with -Zsanitizer=address"
RUSTFLAGS="-Zsanitizer=address" cargo +nightly build -Zbuild-std \
	--target x86_64-unknown-linux-gnu \
	--example r0-smoke --target-dir target/asan

echo "== asan: run + SIGTERM (leaks off: libweston keeps startup allocs)"
ASAN_OPTIONS=detect_leaks=0 \
	target/asan/x86_64-unknown-linux-gnu/debug/examples/r0-smoke \
	> /tmp/r0-asan.log 2>&1 &
PID=$!
sleep 3
kill -0 "$PID" || { cat /tmp/r0-asan.log >&2; echo "FAIL: died at startup"; exit 1; }
kill -TERM "$PID"
wait "$PID" || { cat /tmp/r0-asan.log >&2; echo "FAIL: non-zero exit"; exit 1; }
grep -q "westonite-r0: clean exit (0)" /tmp/r0-asan.log \
	|| { cat /tmp/r0-asan.log >&2; echo "FAIL: no clean-exit marker"; exit 1; }

echo "ASAN SMOKE PASSED"
