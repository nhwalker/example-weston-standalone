srcs_westonite = [
	git_version_h,
	'main.c',
	'text-backend.c',
	'config-helpers.c',
	'weston-screenshooter.c',
	text_input_unstable_v1_server_protocol_h,
	text_input_unstable_v1_protocol_c,
	input_method_unstable_v1_server_protocol_h,
	input_method_unstable_v1_protocol_c,
]
deps_westonite = [
	dep_libshared,
	dep_libweston,
	dep_libinput,
	dep_libevdev,
	dep_libdl,
	dep_threads,
]

if get_option('xwayland')
	srcs_westonite += 'xwayland.c'
endif

libexec_westonite = shared_library(
	'exec_westonite',
	sources: srcs_westonite,
	include_directories: common_inc,
	dependencies: deps_westonite,
	install_dir: dir_module_westonite,
	install: true,
	version: '0.0.0',
	soversion: 0
)
dep_libexec_westonite = declare_dependency(
	link_with: libexec_westonite,
	include_directories: include_directories('.'),
	dependencies: dep_libweston
)

exe_westonite = executable(
	'westonite',
	'executable.c',
	include_directories: common_inc,
	dependencies: dep_libexec_westonite,
	install_rpath: dir_module_westonite,
	install: true
)
