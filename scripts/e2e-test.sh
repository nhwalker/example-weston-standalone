#!/bin/bash
# Build westonite from /src and run the e2e suite (tests/e2e).
# Runs inside the containers/Containerfile.build image, as root.
# Usage: e2e-test.sh [results-dir]   (default: /tmp; CI mounts a host
# dir here to collect the JUnit XML and per-failure compositor logs)
#
# The VNC backend always requires PAM authentication (service
# weston-remote-access) as the user running the compositor, so the
# suite itself runs as a dedicated non-root user with a real password
# checked by pam_unix -- the same stack a production system uses.
set -euo pipefail

RESULTS="${1:-/tmp}"

cd /src
rm -rf build
meson setup build --prefix=/usr -De2e-test-client=true
ninja -C build
ninja -C build install

mkdir -p -m 1777 /tmp/.X11-unix   # Xwayland needs it; bare containers lack it

E2E_USER=e2e
E2E_PASSWORD=westonite-e2e
printf 'auth     required pam_unix.so\naccount  required pam_unix.so\n' \
	> /etc/pam.d/weston-remote-access
id -u "$E2E_USER" >/dev/null 2>&1 || useradd -m "$E2E_USER"
echo "$E2E_USER:$E2E_PASSWORD" | chpasswd

mkdir -p "$RESULTS/failures"
chown -R "$E2E_USER" "$RESULTS/failures"
touch "$RESULTS/e2e-results.xml" && chown "$E2E_USER" "$RESULTS/e2e-results.xml"

exec runuser -u "$E2E_USER" -- env \
	WESTONITE_VNC_USER="$E2E_USER" \
	WESTONITE_VNC_PASSWORD="$E2E_PASSWORD" \
	WTEST_CLIENT=/src/build/tests/e2e/clients/wtest-client \
	WTEST_XCLIENT=/src/build/tests/e2e/clients/wtest-xclient \
	WESTONITE_E2E_ARTIFACTS="$RESULTS/failures" \
	python3 -m pytest /src/tests/e2e -v -p no:cacheprovider \
		--junit-xml="$RESULTS/e2e-results.xml"
