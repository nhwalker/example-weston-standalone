# wtest-client: xdg-shell test driver for the e2e suite.
# Built only with -De2e-test-client=true; never installed.

dep_wayland_client = dependency('wayland-client')
dep_wayland_protocols = dependency('wayland-protocols')
prog_scanner = find_program('wayland-scanner')

dir_wp = dep_wayland_protocols.get_variable(pkgconfig: 'pkgdatadir')
xml_xdg_shell = dir_wp / 'stable' / 'xdg-shell' / 'xdg-shell.xml'

xdg_shell_client_h = custom_target(
	'xdg-shell-client-protocol.h',
	command: [ prog_scanner, 'client-header', '@INPUT@', '@OUTPUT@' ],
	input: xml_xdg_shell,
	output: 'xdg-shell-client-protocol.h',
)
xdg_shell_c = custom_target(
	'xdg-shell-protocol.c',
	command: [ prog_scanner, 'private-code', '@INPUT@', '@OUTPUT@' ],
	input: xml_xdg_shell,
	output: 'xdg-shell-protocol.c',
)

executable(
	'wtest-client',
	[ 'wtest-client.c', xdg_shell_client_h, xdg_shell_c ],
	dependencies: [ dep_wayland_client ],
	install: false,
)
