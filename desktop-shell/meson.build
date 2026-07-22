srcs_shell_desktop = [
	'shell.c',
	weston_desktop_shell_server_protocol_h,
	weston_desktop_shell_protocol_c,
]
deps_shell_desktop = [
	dep_libm,
	dep_libexec_westonite,
	dep_libshared,
	dep_libweston,
	dep_pixman,
]
plugin_shell_desktop = shared_library(
	'desktop-shell',
	srcs_shell_desktop,
	include_directories: common_inc,
	dependencies: deps_shell_desktop,
	name_prefix: '',
	install: true,
	install_dir: dir_module_westonite,
	install_rpath: '$ORIGIN'
)
