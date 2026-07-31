#!/bin/bash
# Run the DRM leg of the e2e suite inside a throwaway VM that has a real
# KMS device (vkms).  See docs/drm-testing.md for why this exists and
# what it does not prove.
#
# Runs inside the containers/Containerfile.drm-vm image, as root.
# Usage: drm-vm-test.sh [results-dir] [frontend]
#   frontend: "c" (default, the oracle) or "rust"
#
# The container needs no privileges.  --device /dev/kvm makes the VM
# roughly 10x faster but its absence only costs wall-clock: qemu falls
# back to TCG.
set -euo pipefail

RESULTS="${1:-/tmp}"
FRONTEND="${2:-c}"
mkdir -p "$RESULTS"

CONSOLE="$RESULTS/drm-vm-console.log"
JUNIT="$RESULTS/e2e-drm-$FRONTEND.xml"

test -d /vm/rootfs || { echo "not the drm-vm image (no /vm/rootfs)" >&2; exit 2; }

# Decided up front because the guest payload needs it too: under TCG the
# compositor takes some 7s to start (llvmpipe EGL init dominates), which
# puts the suite's 10s deadlines right on the edge, so the guest scales
# every deadline rather than the suite loosening them for everyone.
# `if`, not `[ ... ] && ACCEL=kvm`: a bare test that comes out false is a
# failing command, and under `set -e` the absence of /dev/kvm would end
# the script instead of selecting TCG.
ACCEL=tcg
SCALE=4
if [ -c /dev/kvm ] && [ -w /dev/kvm ]; then
	ACCEL=kvm
	SCALE=1
fi

# -- build ---------------------------------------------------------------
cd /src
if [ -f build/build.ninja ]; then
	meson configure build -De2e-test-client=true >/dev/null
else
	meson setup build --prefix=/usr -De2e-test-client=true >/dev/null
fi
ninja -C build >/dev/null
if [ "$FRONTEND" = rust ]; then
	cargo build --locked --release -p westonite
fi

# -- stage the guest root ------------------------------------------------
# A real copy, not hardlinks: the injections below would otherwise write
# through into the baked rootfs and poison every later run.
rm -rf /vm/run /vm/run.img /vm/results.img
cp -a /vm/rootfs /vm/run

# The C frontend and its shell plugin, installed exactly as the RPM
# would (prefix=/usr), so the guest exercises the shipped layout.
DESTDIR=/vm/run ninja -C build install >/dev/null

# The image bakes an init.sh, but take the working tree's copy: editing
# the guest init should not cost a 6-minute image rebuild, and in CI the
# two are the same file from the same commit anyway.
cp /src/containers/drm-vm-init.sh /vm/run/init.sh
chmod +x /vm/run/init.sh

mkdir -p /vm/run/opt/e2e
cp -a /src/tests/e2e/. /vm/run/opt/e2e/
mkdir -p /vm/run/usr/local/bin
if [ "$FRONTEND" = rust ]; then
	cp /src/target/release/westonite-rs /vm/run/usr/local/bin/
fi
cp build/tests/e2e/clients/wtest-client /vm/run/usr/local/bin/ 2>/dev/null || true
cp build/tests/e2e/clients/wtest-xclient /vm/run/usr/local/bin/ 2>/dev/null || true

if [ "$FRONTEND" = rust ]; then
	BIN=/usr/local/bin/westonite-rs
	FORMAT=toml
else
	BIN=/usr/bin/westonite
	FORMAT=ini
fi

# The payload runs as root inside a single-purpose VM.  WESTONITE_DRM_VM
# is what unskips the DRM tests: they must NEVER run on a developer's
# machine just because it happens to have /dev/dri -- taking DRM master
# there would black out their display.
cat > /vm/run/opt/vm-command.sh <<EOF
#!/bin/bash
cd /opt/e2e
export WESTONITE_DRM_VM=1
export WESTONITE_E2E_TIMEOUT_SCALE=$SCALE
export WESTONITE_BIN=$BIN
export WESTONITE_CONFIG_FORMAT=$FORMAT
export WTEST_CLIENT=/usr/local/bin/wtest-client
export WTEST_XCLIENT=/usr/local/bin/wtest-xclient
export WESTONITE_E2E_ARTIFACTS=/results/failures-drm-$FRONTEND
mkdir -p "\$WESTONITE_E2E_ARTIFACTS"
python3 -m pytest /opt/e2e/test_backend_drm.py -v -p no:cacheprovider \\
	--junit-xml=/results/junit.xml
EOF
chmod +x /vm/run/opt/vm-command.sh

# 2G: the staged guest root is ~1 GB (860 MB baked + the injected
# binaries and e2e tree).  Sparse, but CI runners are not generous with
# disk and the image is built alongside a 4 GB build image.
mkfs.ext4 -q -F -L drmvm-root -d /vm/run /vm/run.img 2G
mkfs.ext4 -q -F -L drmvm-results /vm/results.img 64M

# -- boot ----------------------------------------------------------------
echo "drm-vm: booting with -accel $ACCEL, timeout scale $SCALE (frontend: $FRONTEND)"

# -no-reboot so a panic ends the process instead of looping; the guest
# powers itself off with sysrq when it is done.  console=ttyS0 + -serial
# stdio is the only channel out.
set +e
timeout 900 /usr/libexec/qemu-kvm \
	-accel "$ACCEL" -M q35 -cpu max -m 3G -smp 2 -no-reboot \
	-display none -serial stdio \
	-kernel /vm/vmlinuz -initrd /vm/initramfs.img \
	-drive file=/vm/run.img,if=virtio,format=raw \
	-drive file=/vm/results.img,if=virtio,format=raw \
	-append "root=LABEL=drmvm-root rw console=ttyS0,115200 init=/init.sh selinux=0 panic=10 rd.emergency=poweroff" \
	> "$CONSOLE" 2>&1
QEMU_RC=$?
set -e

# -- results -------------------------------------------------------------
# debugfs reads the results filesystem without mounting it, so this stays
# unprivileged.  `|| true` because a guest that died early leaves nothing
# to dump and the console is then the whole story.
debugfs -R "dump /junit.xml $JUNIT" /vm/results.img >/dev/null 2>&1 || true
[ -s "$JUNIT" ] || rm -f "$JUNIT"
debugfs -R "rdump /failures-drm-$FRONTEND $RESULTS" /vm/results.img \
	>/dev/null 2>&1 || true

# The sentinel, not qemu's exit status, is the verdict: qemu exits 0 for
# a guest that panicked, hung until the timeout, or never ran the tests.
# No sentinel is a failure, always.
#
# The guest prints it twice -- once through /dev/kmsg, once to the
# console -- so take the FIRST match, which is the kmsg one: printk
# emits records whole, while the console copy can be spliced by a
# concurrent printk (see containers/drm-vm-init.sh).  And require at
# least one digit, so a console copy cut off right after the `=` cannot
# match and yield an empty verdict.
#
# `|| true`: with pipefail set, a grep that matches nothing -- or a
# SIGPIPE from head -- would take the whole script down here, exactly in
# the case whose error message below is the most useful thing this
# script can produce.
SENTINEL=$(grep -ao 'DRMVM-EXIT=[0-9][0-9]*' "$CONSOLE" | head -1 | cut -d= -f2 || true)
if [ -z "$SENTINEL" ]; then
	echo "drm-vm: FAILED -- no DRMVM-EXIT sentinel (qemu rc=$QEMU_RC)" >&2
	tail -60 "$CONSOLE" >&2
	exit 1
fi
if [ "$SENTINEL" != 0 ]; then
	echo "drm-vm: FAILED -- guest reported exit $SENTINEL" >&2
	tail -80 "$CONSOLE" >&2
fi
echo "drm-vm: guest exit $SENTINEL (console: $CONSOLE)"
exit "$SENTINEL"
