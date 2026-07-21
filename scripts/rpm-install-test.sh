#!/bin/bash
# Install the built RPM into a PRISTINE CentOS Stream 10 container and
# rerun the runtime smoke tests from the installed files.
# Usage: rpm-install-test.sh [rpm-dir]   (default: /rpms)
set -euo pipefail

RPMDIR="${1:-/rpms}"

dnf -y install epel-release
dnf config-manager --set-enabled crb
dnf -y install "$RPMDIR"/westonite-[0-9]*.x86_64.rpm \
	xorg-x11-server-Xwayland xdpyinfo

rpm -q westonite weston-libs
if rpm -q weston >/dev/null 2>&1; then
	echo "FAIL: full weston package was pulled in" >&2
	exit 1
fi

export XDG_RUNTIME_DIR=/tmp/xdg
mkdir -p -m 0700 "$XDG_RUNTIME_DIR"
mkdir -p -m 1777 /tmp/.X11-unix

westonite --backend=headless --xwayland --log=/tmp/w.log &
WPID=$!
sleep 2
DISP=$(grep -oP 'listening on display \K:[0-9]+' /tmp/w.log)
DISPLAY=$DISP xdpyinfo | grep 'vendor string'
kill $WPID
wait $WPID
grep -q "launching" /tmp/w.log && ! grep -q "launching '/usr/bin/Xwayland'" /tmp/w.log \
	&& { echo "FAIL: unexpected client launch" >&2; exit 1; }

echo "RPM INSTALL TEST PASSED"
