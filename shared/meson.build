# Subset of upstream shared/: only the pieces whose symbols are NOT
# exported by libweston-14.so (config-parser is exported and comes from
# the RPM; see docs/phase0-findings.md).
srcs_libshared = [
	'os-compatibility.c',
	'process-util.c',
	'option-parser.c',
]

lib_libshared = static_library(
	'shared',
	srcs_libshared,
	include_directories: common_inc,
	dependencies: [ dep_libweston, dep_wayland_server ],
	pic: true,
	install: false
)
dep_libshared = declare_dependency(
	link_with: lib_libshared,
	dependencies: [ dep_libweston ],
)
