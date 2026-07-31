# Testing the DRM backend

The DRM backend is the last backend westonite has to port (plan §7,
R2c). Unlike every other backend it needs a *kernel* mode-setting
device, which neither a container nor a GitHub-hosted runner gives us
directly. This document records how we establish one, and — just as
importantly — what that does and does not prove.

## 1. What we tried first, and why it was dropped

Before the VM route we ran four throwaway probes in CI (non-gating,
`continue-on-error`) to find out whether the runner itself could host a
`vkms` device. They are gone from `.github/workflows/ci.yml` now; the
facts they established are kept here because they are what the decision
rests on, and re-deriving them costs another four CI rounds.

| # | Question | Answer |
|---|----------|--------|
| 1 | Does the runner kernel have `vkms`? | No. Azure kernel `6.17.0-1020-azure` ships no `vkms.ko` in the base image. |
| 2 | Can it be installed? | Yes — `linux-modules-extra-$(uname -r)` is in the archive and provides it. `sudo modprobe vkms` then succeeds. |
| 3 | Is there already a DRM device? | Yes: `/dev/dri/card1` is `hyperv_drm`, a real KMS device with a *connected* connector. **Off limits** — it is the runner's own console; taking DRM master on it risks disturbing the machine under test. |
| 4 | Is `/dev/kvm` present? | Yes. |
| 5 | How do you *find* the vkms card? | By modprobe delta, not by driver name. Since ~6.14 vkms registers on the **faux bus**, so `/sys/class/drm/cardN/device/driver` resolves to `faux_driver`; a name match silently finds nothing. Probe round 3 fell into exactly this and reported "no vkms card" while its own log showed vkms had loaded. |
| 6 | What about the seat? | libseat on EL10 is compiled with the `logind` and `seatd` backends only — there is no `builtin`, and `LIBSEAT_BACKEND=builtin` fails with *"No backend matched name"*. `logind` wants a session bus and a real login session, which a container has not got. So: run `seatd` and set `LIBSEAT_BACKEND=seatd`. |

The route was abandoned not because any single step failed but because
the shape was wrong: it layered `apt` on the runner, a module load into
the *host* kernel, a device passed into a container, and a daemon, with
every layer's failure mode only observable one CI round at a time. Four
rounds in it was still not green. It is also, by construction,
untestable locally.

## 2. The route we took: a VM we ship whole

    runner ──/dev/kvm──▶ our container ──qemu──▶ VM (our kernel, our rootfs)
                                                   └─ vkms ─ seatd ─ pytest

The kernel travels *inside* our image, so the only thing the harness
asks of the runner is `/dev/kvm` — and its absence degrades to TCG
emulation rather than breaking. Two facts make this cheap:

- CentOS Stream 10's `kernel-modules-core` ships `vkms.ko`
  (kernel 6.12.0-251).
- `qemu-kvm` is in AppStream, so the same base image that builds
  westonite can run the VM.

Pinning the kernel also disposes of finding #5 above: the guest has
exactly one DRM device, the one we loaded, so nothing has to identify
vkms among a machine's other cards — and the kernel version cannot
change under us.

Everything in the guest is built from the same repos as the build
image, so the libweston under test in the VM is the same libweston the
rest of the suite tests against.

### Pieces

| Piece | Where | What it is |
|-------|-------|------------|
| slim rootfs, kernel, initramfs | baked into the VM image at build time | `dnf --installroot` of `kernel-core`, `kernel-modules-core`, `weston-libs`, `seatd`, `python3-pytest` and their deps |
| guest init | `containers/drm-vm-init.sh` | PID 1. Mounts the pseudo filesystems, loads vkms, starts seatd, runs the payload, prints `DRMVM-EXIT=<n>` and powers off. No systemd, no udev. |
| harness | `scripts/drm-vm-test.sh` | Copies the baked rootfs, injects the freshly built binaries and the e2e tree, `mkfs.ext4 -d` (no mount, no privileges), boots qemu, and turns the console sentinel back into an exit code. |

The rootfs is assembled with `mkfs.ext4 -d`, which builds the
filesystem image from a directory *without mounting it* — so the whole
harness runs with no `CAP_SYS_ADMIN`, no loop device and no privileged
container.

The tests themselves are `tests/e2e/test_backend_drm.py`, gated on
`WESTONITE_DRM_VM=1` — see the e2e plan §2.9 for what each one asserts
and why the gate is that variable and not the presence of `/dev/dri`.

### Running it

```sh
docker build -f containers/Containerfile.build  -t westonite-build .
docker build -f containers/Containerfile.drm-vm -t westonite-drm-vm .
docker run --rm --device /dev/kvm \
    -v "$PWD":/src -v "$PWD/test-results":/results \
    westonite-drm-vm /src/scripts/drm-vm-test.sh /results c
```

The last argument selects the frontend: `c` (the oracle) or `rust`.
Drop `--device /dev/kvm` and it still works, on TCG.  Editing
`containers/drm-vm-init.sh` or the tests does *not* need an image
rebuild — the harness injects the working tree's copies over the baked
ones; only changing the guest's package set does.

### Reading the result

The guest's console is the only channel out for progress. The host
greps it for the `DRMVM-EXIT=<n>` sentinel and exits with `n`; **no
sentinel is a failure**, which is what makes a kernel panic, an early
exit and a hang all report correctly instead of silently passing.

JUnit XML and the per-failure artifact directories come back on a
second, tiny ext4 disk, read out with `debugfs dump`/`rdump` — again,
without mounting anything.

### Speed

Without `/dev/kvm` the VM boots in about 35 s and the seven DRM tests
run in about 32 s. That is fast enough that the whole harness is
dominated by staging the guest root (a ~860 MB copy plus `mkfs.ext4
-d`), not by emulation.

TCG *is* slow enough to matter inside the guest, though, and it is the
**first** compositor start that pays: about 22 s, against 1.3–2 s for
every later one in the same run (cold page cache plus llvmpipe EGL
init; once mesa and libweston are cached the emulated CPU keeps up
fine). 22 s against the suite's 10 s deadline is not a near miss, so
the harness exports `WESTONITE_E2E_TIMEOUT_SCALE=4` when it falls back
to TCG and `1` under KVM. Scaling deliberately, rather than loosening
the deadlines for every environment, keeps a real startup regression on
the fast path visible.

## 3. What this does not prove

vkms is a *virtual* KMS device, and it is a better one than expected —
it supports atomic modesetting and GBM modifiers, presents a connected
`Virtual-1` connector alongside a `Writeback-1`, and advertises a
34-entry mode list (1024x768 preferred). Enough that the C frontend
brings a full output up on it with the GL renderer running on llvmpipe.

But those modes are a synthesized standard list, not modes read from a
real display's EDID, and behind them there is no physical vblank, no
hardware timing, no driver quirk, and none of the atomic-commit
failures real drivers produce. Passing on vkms means the DRM code path
is wired up correctly and does not crash; it does not mean westonite
drives a display.

**A one-time validation on real hardware is still required before R3**
(decommissioning the C frontend). That is tracked as a gate in the
plan, not as something this harness discharges.
