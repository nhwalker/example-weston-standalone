#!/bin/bash
# Install the built RPM into a PRISTINE CentOS Stream 10 container,
# rerun the runtime smoke tests from the installed files, and run the
# @pytest.mark.installed subset of the e2e suite against them.
# Usage: rpm-install-test.sh [rpm-dir] [repo-dir] [results-dir]
#        (defaults: /rpms /src /tmp)
set -euo pipefail

RPMDIR="${1:-/rpms}"
SRCDIR="${2:-/src}"
RESULTS="${3:-/tmp}"

dnf -y install epel-release
dnf config-manager --set-enabled crb
dnf -y install "$RPMDIR"/westonite-[0-9]*.x86_64.rpm \
	xorg-x11-server-Xwayland xdpyinfo \
	python3-pytest python3-cryptography desktop-file-utils util-linux

rpm -q westonite weston-libs
if rpm -q weston >/dev/null 2>&1; then
	echo "FAIL: full weston package was pulled in" >&2
	exit 1
fi

echo "== session file is valid and points at real binaries"
desktop-file-validate /usr/share/wayland-sessions/westonite.desktop
EXEC=$(sed -n 's/^Exec=\([^ ]*\).*/\1/p' \
	/usr/share/wayland-sessions/westonite.desktop)
command -v "$EXEC" >/dev/null \
	|| { echo "FAIL: Exec=$EXEC not found in PATH" >&2; exit 1; }

echo "== legacy smoke: headless + Xwayland round-trip"
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

echo "== e2e installed-subset against the RPM files"
E2E_USER=e2e
E2E_PASSWORD=westonite-e2e
printf 'auth     required pam_unix.so\naccount  required pam_unix.so\n' \
	> /etc/pam.d/weston-remote-access
id -u "$E2E_USER" >/dev/null 2>&1 || useradd -m "$E2E_USER"
echo "$E2E_USER:$E2E_PASSWORD" | chpasswd

mkdir -p "$RESULTS/failures-installed"
chown -R "$E2E_USER" "$RESULTS/failures-installed"
touch "$RESULTS/e2e-installed.xml" && chown "$E2E_USER" "$RESULTS/e2e-installed.xml"

runuser -u "$E2E_USER" -- env \
	WESTONITE_VNC_USER="$E2E_USER" \
	WESTONITE_VNC_PASSWORD="$E2E_PASSWORD" \
	WESTONITE_E2E_ARTIFACTS="$RESULTS/failures-installed" \
	python3 -m pytest "$SRCDIR/tests/e2e" -v -m installed \
		-p no:cacheprovider --junit-xml="$RESULTS/e2e-installed.xml"

echo "RPM INSTALL TEST PASSED"
