dep_scanner = dependency('wayland-scanner', native: true)
prog_scanner = find_program(dep_scanner.get_variable(pkgconfig: 'wayland_scanner'))

dep_wp = dependency('wayland-protocols', version: '>= 1.33')
dir_wp_base = dep_wp.get_variable(pkgconfig: 'pkgdatadir')

xml_text_input = dir_wp_base / 'unstable' / 'text-input' / 'text-input-unstable-v1.xml'
xml_input_method = dir_wp_base / 'unstable' / 'input-method' / 'input-method-unstable-v1.xml'
xml_desktop_shell = files('weston-desktop-shell.xml')

text_input_unstable_v1_protocol_c = custom_target(
	'text-input-unstable-v1 protocol.c',
	input: xml_text_input,
	output: 'text-input-unstable-v1-protocol.c',
	command: [ prog_scanner, 'private-code', '@INPUT@', '@OUTPUT@' ],
)
text_input_unstable_v1_server_protocol_h = custom_target(
	'text-input-unstable-v1 server-header.h',
	input: xml_text_input,
	output: 'text-input-unstable-v1-server-protocol.h',
	command: [ prog_scanner, 'server-header', '@INPUT@', '@OUTPUT@' ],
)

input_method_unstable_v1_protocol_c = custom_target(
	'input-method-unstable-v1 protocol.c',
	input: xml_input_method,
	output: 'input-method-unstable-v1-protocol.c',
	command: [ prog_scanner, 'private-code', '@INPUT@', '@OUTPUT@' ],
)
input_method_unstable_v1_server_protocol_h = custom_target(
	'input-method-unstable-v1 server-header.h',
	input: xml_input_method,
	output: 'input-method-unstable-v1-server-protocol.h',
	command: [ prog_scanner, 'server-header', '@INPUT@', '@OUTPUT@' ],
)

weston_desktop_shell_protocol_c = custom_target(
	'weston-desktop-shell protocol.c',
	input: xml_desktop_shell,
	output: 'weston-desktop-shell-protocol.c',
	command: [ prog_scanner, 'private-code', '@INPUT@', '@OUTPUT@' ],
)
weston_desktop_shell_server_protocol_h = custom_target(
	'weston-desktop-shell server-header.h',
	input: xml_desktop_shell,
	output: 'weston-desktop-shell-server-protocol.h',
	command: [ prog_scanner, 'server-header', '@INPUT@', '@OUTPUT@' ],
)
