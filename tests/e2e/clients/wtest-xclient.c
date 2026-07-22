/*
 * wtest-xclient: minimal solid-color X11 client for the westonite e2e
 * suite (Xwayland coverage). EL10 ships no X demo apps (no xeyes,
 * xclock, or xwininfo), so the suite carries its own: a flat-color
 * window for exact pixel assertions that also reports its root-relative
 * position on every ConfigureNotify -- covering what xwininfo would.
 *
 * Test-only: built via -De2e-test-client=true, never installed.
 *
 * stdout protocol (one event per line, flushed):
 *   mapped
 *   position: X Y WxH     (root-relative, after map and every configure)
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <xcb/xcb.h>

static xcb_connection_t *conn;
static xcb_window_t window, root;

static void
report_position(void)
{
	xcb_translate_coordinates_reply_t *trans;
	xcb_get_geometry_reply_t *geom;

	geom = xcb_get_geometry_reply(conn,
				      xcb_get_geometry(conn, window), NULL);
	trans = xcb_translate_coordinates_reply(
		conn,
		xcb_translate_coordinates(conn, window, root, 0, 0), NULL);
	if (!geom || !trans) {
		fprintf(stderr, "geometry query failed\n");
		exit(1);
	}
	printf("position: %d %d %ux%u\n",
	       trans->dst_x, trans->dst_y, geom->width, geom->height);
	fflush(stdout);
	free(geom);
	free(trans);
}

int
main(int argc, char *argv[])
{
	unsigned int width = 200, height = 150;
	uint32_t color = 0xcc00cc; /* magenta */
	xcb_screen_t *screen;
	uint32_t values[2];
	int i;

	for (i = 1; i < argc; i++) {
		if (!strcmp(argv[i], "--size") && i + 1 < argc)
			sscanf(argv[++i], "%ux%u", &width, &height);
		else if (!strcmp(argv[i], "--color") && i + 1 < argc)
			color = strtoul(argv[++i], NULL, 16);
		else {
			fprintf(stderr, "unknown option %s\n", argv[i]);
			return 2;
		}
	}

	conn = xcb_connect(NULL, NULL);
	if (xcb_connection_has_error(conn)) {
		fprintf(stderr, "cannot connect to X display\n");
		return 1;
	}
	screen = xcb_setup_roots_iterator(xcb_get_setup(conn)).data;
	root = screen->root;

	window = xcb_generate_id(conn);
	values[0] = color;
	values[1] = XCB_EVENT_MASK_EXPOSURE | XCB_EVENT_MASK_STRUCTURE_NOTIFY;
	xcb_create_window(conn, XCB_COPY_FROM_PARENT, window, root,
			  0, 0, width, height, 0,
			  XCB_WINDOW_CLASS_INPUT_OUTPUT, screen->root_visual,
			  XCB_CW_BACK_PIXEL | XCB_CW_EVENT_MASK, values);
	xcb_change_property(conn, XCB_PROP_MODE_REPLACE, window,
			    XCB_ATOM_WM_NAME, XCB_ATOM_STRING, 8,
			    strlen("wtest-xclient"), "wtest-xclient");
	xcb_map_window(conn, window);
	xcb_flush(conn);

	while (1) {
		xcb_generic_event_t *event = xcb_wait_for_event(conn);

		if (!event)
			break;
		switch (event->response_type & ~0x80) {
		case XCB_MAP_NOTIFY:
			printf("mapped\n");
			fflush(stdout);
			report_position();
			break;
		case XCB_CONFIGURE_NOTIFY:
			report_position();
			break;
		}
		free(event);
	}
	return 0;
}
