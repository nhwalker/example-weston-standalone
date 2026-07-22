#!/bin/bash
# Build westonite from /src and run the e2e suite (tests/e2e).
# Runs inside the containers/Containerfile.build image, as root.
#
# The VNC backend always requires PAM authentication (service
# weston-remote-access) as the user running the compositor, so the
# suite itself runs as a dedicated non-root user with a real password
# checked by pam_unix -- the same stack a production system uses.
set -euo pipefail

cd /src
rm -rf build
meson setup build --prefix=/usr -De2e-test-client=true
ninja -C build
ninja -C build install

E2E_USER=e2e
E2E_PASSWORD=westonite-e2e
printf 'auth     required pam_unix.so\naccount  required pam_unix.so\n' \
	> /etc/pam.d/weston-remote-access
id -u "$E2E_USER" >/dev/null 2>&1 || useradd -m "$E2E_USER"
echo "$E2E_USER:$E2E_PASSWORD" | chpasswd

exec runuser -u "$E2E_USER" -- env \
	WESTONITE_VNC_USER="$E2E_USER" \
	WESTONITE_VNC_PASSWORD="$E2E_PASSWORD" \
	WTEST_CLIENT=/src/build/tests/e2e/clients/wtest-client \
	python3 -m pytest /src/tests/e2e -v -p no:cacheprovider \
		--junit-xml=/tmp/e2e-results.xml "$@"
