#!/bin/bash
# Build the Rust frontend (westonite-rs) and run the e2e suite against
# it (plan §7 R2a/R2b/R2c/R2d).  Since R2d (xwayland) the Rust frontend
# runs the ENTIRE suite; the C frontend leg (e2e-test.sh) remains the
# oracle only for the not-yet-ported backends.
#
# The suite needs the meson-built test clients (wtest-client) and the
# VNC PAM stack, so this mirrors e2e-test.sh's setup with the Rust
# binary under test (TOML config mode).
# Runs inside the containers/Containerfile.build image, as root.
# Usage: rust-e2e-test.sh [results-dir]
set -euo pipefail

RESULTS="${1:-/tmp}"

cd /src
cargo build --locked --release -p westonite

# Test clients only — the C compositor is not under test here, but the
# suite's wtest clients come out of the meson tree.
if [ -f build/build.ninja ]; then
	meson configure build -De2e-test-client=true >/dev/null
else
	meson setup build --prefix=/usr -De2e-test-client=true >/dev/null
fi
ninja -C build >/dev/null

mkdir -p -m 1777 /tmp/.X11-unix

E2E_USER=e2e
E2E_PASSWORD=westonite-e2e
printf 'auth     required pam_unix.so\naccount  required pam_unix.so\n' \
	> /etc/pam.d/weston-remote-access
id -u "$E2E_USER" >/dev/null 2>&1 || useradd -m "$E2E_USER"
echo "$E2E_USER:$E2E_PASSWORD" | chpasswd

mkdir -p "$RESULTS/failures-rust-frontend"
chown -R "$E2E_USER" "$RESULTS/failures-rust-frontend"
touch "$RESULTS/e2e-rust-frontend.xml" && chown "$E2E_USER" "$RESULTS/e2e-rust-frontend.xml"

exec runuser -u "$E2E_USER" -- env \
	WESTONITE_BIN=/src/target/release/westonite-rs \
	WESTONITE_CONFIG_FORMAT=toml \
	WESTONITE_VNC_USER="$E2E_USER" \
	WESTONITE_VNC_PASSWORD="$E2E_PASSWORD" \
	WTEST_CLIENT=/src/build/tests/e2e/clients/wtest-client \
	WTEST_XCLIENT=/src/build/tests/e2e/clients/wtest-xclient \
	WESTONITE_E2E_ARTIFACTS="$RESULTS/failures-rust-frontend" \
	python3 -m pytest /src/tests/e2e -v -p no:cacheprovider \
		--junit-xml="$RESULTS/e2e-rust-frontend.xml"
