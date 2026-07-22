# westonite

The Weston 14 compositor **frontend** and **desktop-shell** plugin, built
standalone in this repository against the distribution's **libweston 14
RPMs**, and renamed `westonite` so it installs cleanly alongside (or
instead of) the stock `weston` package.

- Upstream base: weston tag `14.0.1` (matching EPEL 10's
  `weston-14.0.1-3.el10_0`) — provenance and local patches in
  [`VENDOR.md`](VENDOR.md).
- Target platform: RHEL 10 / UBI 10 / CentOS Stream 10 with **EPEL 10**
  (RHEL itself ships no weston; research in
  [`docs/phase0-findings.md`](docs/phase0-findings.md)).
- Design and phase history: [`PLAN.md`](PLAN.md).

What you get: `/usr/bin/westonite`, `/usr/lib64/westonite/desktop-shell.so`
(+ `libexec_westonite.so`), a `wayland-sessions/westonite.desktop` entry,
and an example config. Backends, renderers, and the Xwayland module are
**not** built here — they load at runtime from EPEL's `weston-libs`
(`/usr/lib64/libweston-14/`).

By default westonite spawns **no helper clients**: no panel, no background
client, no on-screen keyboard. See
[`data/westonite.ini.example`](data/westonite.ini.example) for how to
point it at EPEL's `weston-desktop-shell`/`weston-keyboard` if you want
them back.

## Required repositories

| Repo | Build | Runtime | Provides |
|---|---|---|---|
| BaseOS/AppStream | ✓ | ✓ | toolchain, wayland, libevdev, Xwayland |
| CRB | ✓ | ✓ | meson, libinput-devel; runtime: libturbojpeg (pulled via weston-libs → neatvnc) |
| EPEL 10 | ✓ | ✓ | `weston-devel` (build), `weston-libs` (runtime) |

On CentOS Stream 10:
`dnf install epel-release && dnf config-manager --set-enabled crb`.
On entitled RHEL 10 / UBI 10 the CRB repo is
`codeready-builder-for-rhel-10-$(arch)-rpms`.

## Building

```sh
# Everything below can run in the ready-made container image:
docker build -f containers/Containerfile.build -t westonite-build .

meson setup build --prefix=/usr
ninja -C build
ninja -C build install
```

## Building the RPM

```sh
git archive --prefix=westonite-14.0.1/ -o westonite-14.0.1.tar.gz HEAD
rpmdev-setuptree   # or: mkdir -p ~/rpmbuild/{SOURCES,SPECS}
cp westonite-14.0.1.tar.gz ~/rpmbuild/SOURCES/
rpmbuild -ba rpm/westonite.spec
dnf install ~/rpmbuild/RPMS/x86_64/westonite-14.0.1-*.rpm
```

## Running

```sh
westonite                          # DRM backend (a real seat/TTY)
westonite --backend=headless       # no display hardware
westonite --xwayland               # enable X11 client support
```

Configuration lives in `westonite.ini` (searched in `$XDG_CONFIG_HOME`,
`~/.config`; format identical to `weston.ini(5)`), or pass
`-c /path/to/file`. Note for minimal containers: `/tmp/.X11-unix` must
exist before starting with `--xwayland` (normal systems create it via
systemd-tmpfiles).

## Testing

All test code lives in this repo and treats the built (or installed)
artifacts strictly black-box; the design, coverage inventory, and the
RPM-stack findings the suite uncovered are documented in
[`docs/e2e-test-plan.md`](docs/e2e-test-plan.md).

```sh
# fast sanity gate (headless + Xwayland smoke)
docker run --rm -v "$PWD":/src westonite-build /src/scripts/smoke-test.sh

# full e2e suite (~45 tests, ~20 s after the build)
docker run --rm -v "$PWD":/src westonite-build /src/scripts/e2e-test.sh
```

How it works: each pytest test boots its own westonite instance and
drives it through the **VNC backend** — one authenticated RFB
connection provides scripted keyboard/pointer injection *and*
framebuffer capture. The RFB client is in-repo, pure Python
(`tests/e2e/support/vncclient.py`, Apple-DH auth via
`python3-cryptography`) because EPEL's neatvnc offers no security type
that off-the-shelf scriptable clients speak. Auth runs through a real
`pam_unix` stack (`/etc/pam.d/weston-remote-access`) as a dedicated
non-root user. Windows come from two test-only clients built with
`-De2e-test-client=true` (never installed, not in the RPM):
`wtest-client` (xdg-shell, solid-color, turns clicks into
move/resize requests) and `wtest-xclient` (xcb, for Xwayland). Every
scene is flat-colored, so pixel assertions are exact with no reference
images to maintain, and every wait polls a condition — no sleeps on
the happy path.

CI runs the suite on every push (see `.github/workflows/ci.yml`),
plus an 8-test `@pytest.mark.installed` subset re-run against the
installed RPM inside a pristine container. JUnit XML and per-failure
compositor logs are uploaded as the `test-results` artifact.

## License

MIT/Expat, same as upstream Weston — see [`COPYING`](COPYING).
