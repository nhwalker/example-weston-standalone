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

## License

MIT/Expat, same as upstream Weston — see [`COPYING`](COPYING).
