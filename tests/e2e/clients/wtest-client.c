/*
 * wtest-client: minimal xdg-shell test client for the westonite e2e suite.
 *
 * Draws a solid-color toplevel (exact pixels -- no fonts, no toolkit),
 * reports protocol events on stdout as single lines, and turns pointer
 * clicks into interactive move/resize requests so the suite can exercise
 * the shell's grab machinery through VNC input injection.
 *
 * Test-only: built via -De2e-test-client=true, never installed.
 *
 * stdout protocol (one event per line, flushed):
 *   wm-capabilities: [name ...]
 *   configure: WxH [state ...]
 *   mapped: WxH
 *   focus: enter | leave
 *   pointer: enter | button PRESSED
 *   paused / resumed        (SIGUSR1 / SIGUSR2)
 *
 * SIGUSR1 stops display dispatching (simulates an unresponsive client,
 * including unanswered pings); SIGUSR2 resumes.
 */

#define _GNU_SOURCE

#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <signal.h>
#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

#include <wayland-client.h>
#include "xdg-shell-client-protocol.h"

struct app {
	struct wl_display *display;
	struct wl_compositor *compositor;
	struct wl_shm *shm;
	struct wl_seat *seat;
	struct wl_pointer *pointer;
	struct wl_keyboard *keyboard;
	struct xdg_wm_base *wm_base;

	struct wl_surface *surface;
	struct xdg_surface *xdg_surface;
	struct xdg_toplevel *toplevel;

	int width, height;
	uint32_t color, focus_color;
	bool have_focus;
	bool interactive; /* left-click: move grab; right-click: resize grab */
	bool request_fullscreen, request_maximized;
	bool configured, mapped;
	bool running;
};

static volatile sig_atomic_t paused;

static void
say(const char *fmt, ...)
{
	va_list ap;

	va_start(ap, fmt);
	vprintf(fmt, ap);
	va_end(ap);
	putchar('\n');
	fflush(stdout);
}

static void
buffer_release(void *data, struct wl_buffer *buffer)
{
	wl_buffer_destroy(buffer);
}

static const struct wl_buffer_listener buffer_listener = {
	.release = buffer_release,
};

static void
draw(struct app *app)
{
	int stride = app->width * 4;
	int size = stride * app->height;
	uint32_t color = (app->have_focus && app->focus_color) ?
			 app->focus_color : app->color;
	int fd;
	uint32_t *pixels;
	struct wl_shm_pool *pool;
	struct wl_buffer *buffer;
	int i;

	fd = memfd_create("wtest-client", MFD_CLOEXEC);
	if (fd < 0 || ftruncate(fd, size) < 0) {
		perror("shm");
		exit(1);
	}
	pixels = mmap(NULL, size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
	if (pixels == MAP_FAILED) {
		perror("mmap");
		exit(1);
	}
	for (i = 0; i < app->width * app->height; i++)
		pixels[i] = color;
	munmap(pixels, size);

	pool = wl_shm_create_pool(app->shm, fd, size);
	buffer = wl_shm_pool_create_buffer(pool, 0, app->width, app->height,
					   stride, WL_SHM_FORMAT_ARGB8888);
	wl_shm_pool_destroy(pool);
	close(fd);

	wl_buffer_add_listener(buffer, &buffer_listener, NULL);
	wl_surface_attach(app->surface, buffer, 0, 0);
	wl_surface_damage_buffer(app->surface, 0, 0, app->width, app->height);
	wl_surface_commit(app->surface);
}

/* -- xdg_wm_base ----------------------------------------------------- */

static void
wm_base_ping(void *data, struct xdg_wm_base *wm_base, uint32_t serial)
{
	xdg_wm_base_pong(wm_base, serial);
}

static const struct xdg_wm_base_listener wm_base_listener = {
	.ping = wm_base_ping,
};

/* -- xdg_surface / xdg_toplevel -------------------------------------- */

static void
xdg_surface_configure(void *data, struct xdg_surface *surface,
		      uint32_t serial)
{
	struct app *app = data;

	xdg_surface_ack_configure(surface, serial);
	app->configured = true;
	draw(app);
	if (!app->mapped) {
		app->mapped = true;
		say("mapped: %dx%d", app->width, app->height);
	}
}

static const struct xdg_surface_listener xdg_surface_listener = {
	.configure = xdg_surface_configure,
};

static const char *
state_name(uint32_t state)
{
	switch (state) {
	case XDG_TOPLEVEL_STATE_MAXIMIZED: return "maximized";
	case XDG_TOPLEVEL_STATE_FULLSCREEN: return "fullscreen";
	case XDG_TOPLEVEL_STATE_RESIZING: return "resizing";
	case XDG_TOPLEVEL_STATE_ACTIVATED: return "activated";
	default: return "other";
	}
}

static void
toplevel_configure(void *data, struct xdg_toplevel *toplevel,
		   int32_t width, int32_t height, struct wl_array *states)
{
	struct app *app = data;
	uint32_t *state;
	char buf[256] = "";

	if (width > 0 && height > 0) {
		app->width = width;
		app->height = height;
	}
	wl_array_for_each(state, states) {
		strncat(buf, " ", sizeof(buf) - strlen(buf) - 1);
		strncat(buf, state_name(*state), sizeof(buf) - strlen(buf) - 1);
	}
	say("configure: %dx%d [%s ]", width, height, buf);
}

static void
toplevel_close(void *data, struct xdg_toplevel *toplevel)
{
	struct app *app = data;

	say("close-requested");
	app->running = false;
}

static void
toplevel_wm_capabilities(void *data, struct xdg_toplevel *toplevel,
			 struct wl_array *caps)
{
	uint32_t *cap;
	char buf[256] = "";

	wl_array_for_each(cap, caps) {
		char one[32];
		snprintf(one, sizeof(one), " %u", *cap);
		strncat(buf, one, sizeof(buf) - strlen(buf) - 1);
	}
	say("wm-capabilities: [%s ]", buf);
}

static void
toplevel_configure_bounds(void *data, struct xdg_toplevel *toplevel,
			  int32_t w, int32_t h)
{
}

static const struct xdg_toplevel_listener toplevel_listener = {
	.configure = toplevel_configure,
	.close = toplevel_close,
	.configure_bounds = toplevel_configure_bounds,
	.wm_capabilities = toplevel_wm_capabilities,
};

/* -- input ----------------------------------------------------------- */

static void
pointer_enter(void *data, struct wl_pointer *pointer, uint32_t serial,
	      struct wl_surface *surface, wl_fixed_t x, wl_fixed_t y)
{
	say("pointer: enter");
}

static void
pointer_leave(void *data, struct wl_pointer *pointer, uint32_t serial,
	      struct wl_surface *surface)
{
}

static void
pointer_motion(void *data, struct wl_pointer *pointer, uint32_t time,
	       wl_fixed_t x, wl_fixed_t y)
{
}

static void
pointer_button(void *data, struct wl_pointer *pointer, uint32_t serial,
	       uint32_t time, uint32_t button, uint32_t state)
{
	struct app *app = data;

	if (state != WL_POINTER_BUTTON_STATE_PRESSED)
		return;
	say("pointer: button %u", button);
	if (!app->interactive)
		return;
	if (button == 0x110) /* BTN_LEFT */
		xdg_toplevel_move(app->toplevel, app->seat, serial);
	else if (button == 0x111) /* BTN_RIGHT */
		xdg_toplevel_resize(app->toplevel, app->seat, serial,
				    XDG_TOPLEVEL_RESIZE_EDGE_BOTTOM_RIGHT);
}

static void
pointer_axis(void *data, struct wl_pointer *pointer, uint32_t time,
	     uint32_t axis, wl_fixed_t value)
{
}

static void
pointer_frame(void *data, struct wl_pointer *pointer)
{
}

static void
pointer_dummy_u32(void *data, struct wl_pointer *pointer, uint32_t v)
{
}

static void
pointer_axis_stop(void *data, struct wl_pointer *pointer, uint32_t time,
		  uint32_t axis)
{
}

static void
pointer_axis_discrete(void *data, struct wl_pointer *pointer,
		      uint32_t axis, int32_t discrete)
{
}

static void
pointer_axis_value120(void *data, struct wl_pointer *pointer,
		      uint32_t axis, int32_t value120)
{
}

static void
pointer_axis_relative_direction(void *data, struct wl_pointer *pointer,
				uint32_t axis, uint32_t direction)
{
}

static const struct wl_pointer_listener pointer_listener = {
	.enter = pointer_enter,
	.leave = pointer_leave,
	.motion = pointer_motion,
	.button = pointer_button,
	.axis = pointer_axis,
	.frame = pointer_frame,
	.axis_source = pointer_dummy_u32,
	.axis_stop = pointer_axis_stop,
	.axis_discrete = pointer_axis_discrete,
	.axis_value120 = pointer_axis_value120,
	.axis_relative_direction = pointer_axis_relative_direction,
};

static void
keyboard_keymap(void *data, struct wl_keyboard *keyboard, uint32_t format,
		int32_t fd, uint32_t size)
{
	close(fd);
}

static void
keyboard_enter(void *data, struct wl_keyboard *keyboard, uint32_t serial,
	       struct wl_surface *surface, struct wl_array *keys)
{
	struct app *app = data;

	app->have_focus = true;
	say("focus: enter");
	if (app->configured)
		draw(app);
}

static void
keyboard_leave(void *data, struct wl_keyboard *keyboard, uint32_t serial,
	       struct wl_surface *surface)
{
	struct app *app = data;

	app->have_focus = false;
	say("focus: leave");
	if (app->configured)
		draw(app);
}

static void
keyboard_key(void *data, struct wl_keyboard *keyboard, uint32_t serial,
	     uint32_t time, uint32_t key, uint32_t state)
{
	if (state == WL_KEYBOARD_KEY_STATE_PRESSED)
		say("key: %u", key);
}

static void
keyboard_modifiers(void *data, struct wl_keyboard *keyboard, uint32_t serial,
		   uint32_t depressed, uint32_t latched, uint32_t locked,
		   uint32_t group)
{
}

static void
keyboard_repeat_info(void *data, struct wl_keyboard *keyboard,
		     int32_t rate, int32_t delay)
{
}

static const struct wl_keyboard_listener keyboard_listener = {
	.keymap = keyboard_keymap,
	.enter = keyboard_enter,
	.leave = keyboard_leave,
	.key = keyboard_key,
	.modifiers = keyboard_modifiers,
	.repeat_info = keyboard_repeat_info,
};

static void
seat_capabilities(void *data, struct wl_seat *seat, uint32_t caps)
{
	struct app *app = data;

	if ((caps & WL_SEAT_CAPABILITY_POINTER) && !app->pointer) {
		app->pointer = wl_seat_get_pointer(seat);
		wl_pointer_add_listener(app->pointer, &pointer_listener, app);
	}
	if ((caps & WL_SEAT_CAPABILITY_KEYBOARD) && !app->keyboard) {
		app->keyboard = wl_seat_get_keyboard(seat);
		wl_keyboard_add_listener(app->keyboard, &keyboard_listener,
					 app);
	}
}

static void
seat_name(void *data, struct wl_seat *seat, const char *name)
{
}

static const struct wl_seat_listener seat_listener = {
	.capabilities = seat_capabilities,
	.name = seat_name,
};

/* -- registry -------------------------------------------------------- */

static void
registry_global(void *data, struct wl_registry *registry, uint32_t name,
		const char *interface, uint32_t version)
{
	struct app *app = data;

	if (!strcmp(interface, wl_compositor_interface.name)) {
		app->compositor = wl_registry_bind(registry, name,
						   &wl_compositor_interface, 4);
	} else if (!strcmp(interface, wl_shm_interface.name)) {
		app->shm = wl_registry_bind(registry, name,
					    &wl_shm_interface, 1);
	} else if (!strcmp(interface, wl_seat_interface.name)) {
		app->seat = wl_registry_bind(registry, name,
					     &wl_seat_interface,
					     version < 5 ? version : 5);
		wl_seat_add_listener(app->seat, &seat_listener, app);
	} else if (!strcmp(interface, xdg_wm_base_interface.name)) {
		/* v5+ for wm_capabilities */
		app->wm_base = wl_registry_bind(registry, name,
						&xdg_wm_base_interface,
						version < 6 ? version : 6);
		xdg_wm_base_add_listener(app->wm_base, &wm_base_listener,
					 app);
	}
}

static void
registry_global_remove(void *data, struct wl_registry *registry,
		       uint32_t name)
{
}

static const struct wl_registry_listener registry_listener = {
	.global = registry_global,
	.global_remove = registry_global_remove,
};

/* -- main ------------------------------------------------------------ */

static void
handle_pause(int sig)
{
	paused = (sig == SIGUSR1);
}

static uint32_t
parse_color(const char *hex)
{
	return (uint32_t)strtoul(hex, NULL, 16);
}

int
main(int argc, char *argv[])
{
	struct app app = {
		.width = 200,
		.height = 150,
		.color = 0xffcc0000,
		.focus_color = 0,
		.running = true,
	};
	const char *title = "wtest-client";
	struct wl_registry *registry;
	struct sigaction sa = { .sa_handler = handle_pause };
	int i;

	for (i = 1; i < argc; i++) {
		if (!strcmp(argv[i], "--size") && i + 1 < argc)
			sscanf(argv[++i], "%dx%d", &app.width, &app.height);
		else if (!strcmp(argv[i], "--color") && i + 1 < argc)
			app.color = parse_color(argv[++i]);
		else if (!strcmp(argv[i], "--focus-color") && i + 1 < argc)
			app.focus_color = parse_color(argv[++i]);
		else if (!strcmp(argv[i], "--interactive"))
			app.interactive = true;
		else if (!strcmp(argv[i], "--title") && i + 1 < argc)
			title = argv[++i];
		else if (!strcmp(argv[i], "--request-fullscreen"))
			app.request_fullscreen = true;
		else if (!strcmp(argv[i], "--request-maximized"))
			app.request_maximized = true;
		else {
			fprintf(stderr, "unknown option %s\n", argv[i]);
			return 2;
		}
	}

	sigaction(SIGUSR1, &sa, NULL);
	sigaction(SIGUSR2, &sa, NULL);

	app.display = wl_display_connect(NULL);
	if (!app.display) {
		fprintf(stderr, "cannot connect to wayland display\n");
		return 1;
	}
	registry = wl_display_get_registry(app.display);
	wl_registry_add_listener(registry, &registry_listener, &app);
	wl_display_roundtrip(app.display);
	if (!app.compositor || !app.shm || !app.wm_base) {
		fprintf(stderr, "missing globals\n");
		return 1;
	}

	app.surface = wl_compositor_create_surface(app.compositor);
	app.xdg_surface = xdg_wm_base_get_xdg_surface(app.wm_base,
						      app.surface);
	xdg_surface_add_listener(app.xdg_surface, &xdg_surface_listener,
				 &app);
	app.toplevel = xdg_surface_get_toplevel(app.xdg_surface);
	xdg_toplevel_add_listener(app.toplevel, &toplevel_listener, &app);
	xdg_toplevel_set_title(app.toplevel, title);
	xdg_toplevel_set_app_id(app.toplevel, "org.westonite.wtest-client");
	if (app.request_fullscreen)
		xdg_toplevel_set_fullscreen(app.toplevel, NULL);
	if (app.request_maximized)
		xdg_toplevel_set_maximized(app.toplevel);
	wl_surface_commit(app.surface);

	/* poll-based loop (not wl_display_dispatch, which restarts its
	 * internal poll on EINTR) so SIGUSR1 pauses promptly */
	while (app.running) {
		struct pollfd pfd = {
			.fd = wl_display_get_fd(app.display),
			.events = POLLIN,
		};

		if (paused) {
			/* unresponsive mode: stop dispatching entirely */
			say("paused");
			while (paused)
				usleep(100 * 1000);
			say("resumed");
		}
		while (wl_display_prepare_read(app.display) != 0)
			wl_display_dispatch_pending(app.display);
		wl_display_flush(app.display);
		if (poll(&pfd, 1, 200) > 0)
			wl_display_read_events(app.display);
		else
			wl_display_cancel_read(app.display);
		if (wl_display_dispatch_pending(app.display) < 0)
			break;
	}
	return 0;
}
