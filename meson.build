project('westonite',
	'c',
	version: '14.0.1',
	default_options: [
		'warning_level=3',
		'c_std=gnu99',
		'b_lundef=true',
	],
	meson_version: '>= 0.63.0',
	license: 'MIT/Expat',
)

version_westonite = meson.project_version()

dir_prefix = get_option('prefix')
dir_bin = dir_prefix / get_option('bindir')
dir_data = dir_prefix / get_option('datadir')
dir_lib = dir_prefix / get_option('libdir')
dir_libexec = dir_prefix / get_option('libexecdir')
dir_module_westonite = dir_lib / 'westonite'

common_inc = include_directories('.')

pkgconfig = import('pkgconfig')

git_version_h = vcs_tag(
	input: 'git-version.h.meson',
	output: 'git-version.h',
	fallback: version_westonite
)

config_h = configuration_data()

cc = meson.get_compiler('c')

global_args = cc.get_supported_arguments(
	'-Wmissing-prototypes',
	'-Wno-unused-parameter',
	'-Wno-shift-negative-value',
	'-Wno-missing-field-initializers',
	'-Wno-pedantic',
	'-Wundef',
	'-fvisibility=hidden',
)
add_project_arguments(global_args, language: 'c')

if cc.has_header_symbol('sys/sysmacros.h', 'major')
	config_h.set('MAJOR_IN_SYSMACROS', 1)
elif cc.has_header_symbol('sys/mkdev.h', 'major')
	config_h.set('MAJOR_IN_MKDEV', 1)
endif

optional_libc_funcs = [
	'mkostemp', 'strchrnul', 'initgroups', 'posix_fallocate',
	'memfd_create', 'unreachable',
]
foreach func : optional_libc_funcs
	if cc.has_function(func)
		config_h.set('HAVE_' + func.to_upper(), 1)
	endif
endforeach

optional_builtins = {
	'builtin_clz': 'return __builtin_clz(1);',
	'builtin_bswap32': 'return __builtin_bswap32(0);',
	'builtin_popcount': 'return __builtin_popcount(0);',
}
foreach name, check : optional_builtins
	if cc.links('int main(void) { @0@ }'.format(check), name: name)
		config_h.set('HAVE_' + name.to_upper(), 1)
	endif
endforeach

config_h.set('_GNU_SOURCE', '1')
config_h.set('_ALL_SOURCE', '1')

config_h.set_quoted('PACKAGE_STRING', 'westonite @0@'.format(version_westonite))
config_h.set_quoted('PACKAGE_VERSION', version_westonite)
config_h.set_quoted('VERSION', version_westonite)
config_h.set_quoted('PACKAGE_URL', 'https://github.com/nhwalker/example-weston-standalone')
config_h.set_quoted('PACKAGE_BUGREPORT', 'https://github.com/nhwalker/example-weston-standalone/issues')

config_h.set_quoted('BINDIR', dir_bin)
config_h.set_quoted('DATADIR', dir_data)
config_h.set_quoted('LIBEXECDIR', dir_libexec)
config_h.set_quoted('MODULEDIR', dir_module_westonite)

dep_libweston = dependency('libweston-14', version: '>= 14.0.1')

# libweston's own loadable modules (backends, renderers, xwayland.so) come
# from the weston-libs RPM; the frontend loads xwayland.so from there.
dir_module_libweston = dep_libweston.get_variable(pkgconfig: 'libdir') / 'libweston-14'
config_h.set_quoted('LIBWESTON_MODULEDIR', dir_module_libweston)

# The EPEL 10 libweston build ships every backend header (verified in
# Phase 0); probe each anyway so a repackaged libweston fails loudly at
# configure time instead of at compile time.
foreach backend : [ 'drm', 'headless', 'pipewire', 'rdp', 'vnc', 'wayland', 'x11' ]
	if cc.has_header('libweston/backend-@0@.h'.format(backend), dependencies: dep_libweston)
		config_h.set('BUILD_@0@_COMPOSITOR'.format(backend.to_upper()), '1')
	else
		error('libweston-14 does not install backend-@0@.h; frontend/main.c expects all backend headers (see docs/phase0-findings.md).'.format(backend))
	endif
endforeach

backend_default = get_option('backend-default')
config_h.set_quoted('WESTON_NATIVE_BACKEND', backend_default)

if get_option('xwayland')
	config_h.set('BUILD_XWAYLAND', '1')
	config_h.set_quoted('XSERVER_PATH', get_option('xwayland-path'))
	dep_xwayland = dependency('xwayland', required: false)
	if dep_xwayland.found() and dep_xwayland.get_variable(pkgconfig: 'have_listenfd', default_value: 'false') == 'true'
		config_h.set('HAVE_XWAYLAND_LISTENFD', '1')
	endif
endif

dep_wayland_server = dependency('wayland-server')
dep_pixman = dependency('pixman-1')
dep_libinput = dependency('libinput')
dep_libevdev = dependency('libevdev')
dep_libdl = cc.find_library('dl', required: false)
dep_libm = cc.find_library('m', required: false)
dep_threads = dependency('threads')

subdir('shared')
subdir('frontend')
subdir('desktop-shell')
subdir('data')
if get_option('e2e-test-client')
	subdir('tests/e2e/clients')
endif

configure_file(output: 'config.h', configuration: config_h)
