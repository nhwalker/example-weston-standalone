#!/bin/bash
# Guest init for the DRM test VM (docs/drm-testing.md).
#
# This runs as PID 1 -- the kernel is booted with init=/init.sh, so
# there is no systemd, no udev and no login.  Everything the tests need
# is set up here by hand:
#
#   pseudo filesystems -> vkms -> udevd -> seatd -> pytest -> poweroff
#
# devtmpfs is what creates /dev/dri/cardN when vkms registers, so nothing
# in the DRM path itself needs udev.  udevd runs anyway, for exactly one
# reason: libinput's udev backend enumerates input devices with
# `udev_enumerate_add_match_property(e, "ID_INPUT", "1")`, and that
# property is set by udevd's input_id builtin.  With no udevd run, the
# virtio keyboard and tablet the harness attaches exist as
# /dev/input/eventN but are invisible to libinput -- and the
# `[libinput]` configure_device hook never fires.
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

    # The sentinel goes out through /dev/kmsg, not just /dev/console.
    # A userspace write to the console is buffered by the tty layer and
    # flushed asynchronously, so a kernel printk can be emitted into
    # the MIDDLE of it -- which is exactly what the sysrq power-off two
    # lines down did the first time this ran on a KVM-accelerated
    # runner, fast enough for the race to land:
    #
    #   DRMVM: DRMVM-EXI[    6.082274] sysrq: Power Off
    #   T=0
    #
    # and the host rightly reported no sentinel for a run whose seven
    # tests had all passed.  printk emits records whole, so a kmsg
    # write cannot be split that way; <2> (KERN_CRIT) makes sure it
    # reaches the console whatever the console loglevel is.  The
    # console copy stays as the human-readable one -- if it survives
    # intact the host just reads the same value twice.
    echo "<2>DRMVM-EXIT=$code" > /dev/kmsg 2>/dev/null || true
    log "DRMVM-EXIT=$code"

    # Let the tty drain before anything else prints, so the console log
    # stays readable for whoever has to debug a failure.
    sleep 1

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

# -- udev ---------------------------------------------------------------
# Only for the input-device properties (see the header).  --daemon
# forks and returns; `udevadm trigger` then replays an "add" for
# everything already in /sys, because the devices were created before
# udevd started and the kernel does not resend those uevents.
if ! /usr/lib/systemd/systemd-udevd --daemon > /run/udevd.log 2>&1; then
    log "FATAL: systemd-udevd failed to start"
    cat /run/udevd.log 2>&1 || true
    finish 94
fi
udevadm trigger --type=devices --action=add > /dev/null 2>&1 || true
# A bounded settle: a timeout here is not fatal on its own -- the check
# below is what decides, and it says something far more useful.
udevadm settle --timeout=30 > /dev/null 2>&1 || true

# Assert the one thing udevd was installed for.  Without it the libinput
# tests would not fail, they would pass vacuously (no devices, no hook
# calls, no log lines to contradict).
tagged=$(udevadm trigger --type=devices --subsystem-match=input --dry-run \
         --property-match=ID_INPUT=1 --verbose 2>/dev/null | wc -l)
log "udev: $tagged input devices tagged ID_INPUT"
if [ "$tagged" -eq 0 ]; then
    log "FATAL: no ID_INPUT devices -- libinput would see nothing"
    ls -l /dev/input 2>&1 || true
    finish 95
fi

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
