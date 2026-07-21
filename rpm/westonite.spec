Name:           westonite
Version:        14.0.1
Release:        1%{?dist}
Summary:        Standalone Weston-based Wayland compositor

# Vendored weston sources; see VENDOR.md
License:        MIT
URL:            https://github.com/nhwalker/example-weston-standalone
Source0:        %{name}-%{version}.tar.gz

BuildRequires:  gcc
BuildRequires:  meson >= 0.63
BuildRequires:  pkgconfig(libweston-14) >= 14.0.1
BuildRequires:  pkgconfig(wayland-server)
BuildRequires:  pkgconfig(wayland-scanner)
BuildRequires:  pkgconfig(wayland-protocols) >= 1.33
BuildRequires:  pkgconfig(libinput)
BuildRequires:  pkgconfig(libevdev)
BuildRequires:  pkgconfig(pixman-1)
# Enables HAVE_XWAYLAND_LISTENFD; the build works without it
BuildRequires:  pkgconfig(xwayland)

# libweston runtime, backends, renderers and xwayland.so (EPEL 10)
Requires:       weston-libs%{?_isa} >= 14.0.1
Recommends:     xorg-x11-server-Xwayland

%description
Westonite is the Weston 14 compositor frontend and desktop-shell plugin,
built standalone against the distribution's libweston 14 packages and
renamed so it installs alongside the stock weston package. It ships no
helper clients: there is no panel or on-screen keyboard unless configured
to use external ones (see westonite.ini.example).

%prep
%autosetup

%build
%meson
%meson_build

%install
%meson_install

%files
%license COPYING
%doc VENDOR.md
%{_bindir}/westonite
%dir %{_libdir}/westonite
%{_libdir}/westonite/desktop-shell.so
%{_libdir}/westonite/libexec_westonite.so
%{_libdir}/westonite/libexec_westonite.so.*
%{_datadir}/wayland-sessions/westonite.desktop
%{_datadir}/doc/westonite/westonite.ini.example

%changelog
* Tue Jul 21 2026 Nathan Walker <nathan.h.walker@gmail.com> - 14.0.1-1
- Initial package: weston 14.0.1 frontend + desktop-shell built against
  EPEL 10 libweston-14 (weston-libs), renamed to westonite, no helper
  clients, Xwayland enabled.
