#!/bin/bash
# Guest init for the DRM test VM (docs/drm-testing.md).
#
# This runs as PID 1 -- the kernel is booted with init=/init.sh, so
# there is no systemd, no udev and no login.  Everything the tests need
# is set up here by hand:
#
#   pseudo filesystems -> vkms -> seatd -> pytest -> poweroff
#
# devtmpfs is what creates /dev/dri/cardN when vkms registers; udev is
# not involved and is deliberately absent (one less moving part, and
# nothing in the DRM path needs the device nodes renamed or permissioned
# -- the tests run as root).
#
# The last line the host looks for is the DRMVM-EXIT= sentinel: a
# kernel panic, an early exit or a hang all produce *no* sentinel, which
# the host harness treats as failure.  Never make it unconditional.

set -u

# PID 1 inherits no environment at all -- not even PATH -- so bash falls
# back to its compiled-in default, which does not include /usr/sbin.
# modprobe and seatd both live there.
export PATH=/usr/local/bin:/usr/local/sbin:/usr/bin:/usr/sbin

log() { echo "DRMVM: $*" > /dev/console; }

mount -t proc     proc     /proc
mount -t sysfs    sysfs    /sys
mount -t devtmpfs devtmpfs /dev
mkdir -p /dev/pts /dev/shm /run /tmp
mount -t devpts devpts /dev/pts
mount -t tmpfs  tmpfs  /dev/shm
mount -t tmpfs  tmpfs  /run
mount -t tmpfs  tmpfs  /tmp

exec > /dev/console 2>&1

log "guest up: $(uname -r)"

finish() {
    code=$1
    sync
    umount /results 2>/dev/null || true
    log "DRMVM-EXIT=$code"
    # No systemd, so no `poweroff`: ask the kernel directly.  `o` is
    # power-off; if the emulated machine ignores it the host's timeout
    # is the backstop.
    echo 1 > /proc/sys/kernel/sysrq 2>/dev/null || true
    echo o > /proc/sysrq-trigger 2>/dev/null || true
    sleep 15
    exit "$code"
}

# -- vkms ---------------------------------------------------------------
# The whole point of the VM: a KMS device we fully control.  Loading it
# here rather than baking it into the initramfs keeps the failure
# visible on the console instead of in a boot loop.
if ! modprobe vkms; then
    log "FATAL: modprobe vkms failed"
    finish 90
fi

for _ in $(seq 1 50); do
    [ -e /dev/dri/card0 ] && break
    sleep 0.1
done
if [ ! -e /dev/dri/card0 ]; then
    log "FATAL: vkms loaded but no /dev/dri/card0"
    ls -l /dev/dri 2>&1 || true
    finish 91
fi

log "drm devices: $(ls /dev/dri | tr '\n' ' ')"
for con in /sys/class/drm/card*-*; do
    [ -e "$con/status" ] || continue
    log "connector $(basename "$con") = $(cat "$con/status")"
done

# -- seat ---------------------------------------------------------------
# libseat on EL10 is built with the logind and seatd backends only.
# logind wants a session bus and a real login session; seatd is a
# 200-line daemon that just hands out device fds, which is exactly what
# a single-purpose VM wants.
seatd -g root > /run/seatd.log 2>&1 &
for _ in $(seq 1 50); do
    [ -S /run/seatd.sock ] && break
    sleep 0.1
done
if [ ! -S /run/seatd.sock ]; then
    log "FATAL: seatd did not create its socket"
    cat /run/seatd.log 2>&1 || true
    finish 92
fi
export LIBSEAT_BACKEND=seatd

# -- results disk -------------------------------------------------------
# /dev/vdb is a small ext4 image the host made and reads back afterwards
# with `debugfs -R dump` -- so the JUnit XML gets out without the host
# ever mounting anything (no loop device, no CAP_SYS_ADMIN).
# By label, not by /dev/vdN: there is no udev here to make
# /dev/disk/by-label, but libblkid scans devtmpfs directly, and a label
# does not care what order the host passed the -drive arguments in.
mkdir -p /results
if ! mount -L drmvm-results /results; then
    log "WARNING: could not mount the results disk -- console only"
fi

# -- environment --------------------------------------------------------
mkdir -p -m 0700 /run/xdg
export XDG_RUNTIME_DIR=/run/xdg
export HOME=/root
export LD_LIBRARY_PATH=/usr/local/lib64

# The host injects the payload here: binaries under /usr/local, the e2e
# tree under /opt/e2e, and the command to run in /opt/vm-command.sh.
if [ ! -x /opt/vm-command.sh ]; then
    log "FATAL: no /opt/vm-command.sh -- host injection failed"
    finish 93
fi

log "running payload"
/opt/vm-command.sh
finish $?
