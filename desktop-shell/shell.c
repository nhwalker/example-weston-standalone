/*
 * Copyright © 2010-2012 Intel Corporation
 * Copyright © 2011-2012 Collabora, Ltd.
 * Copyright © 2013 Raspberry Pi Foundation
 *
 * Permission is hereby granted, free of charge, to any person obtaining a
 * copy of this software and associated documentation files (the "Software"),
 * to deal in the Software without restriction, including without limitation
 * the rights to use, copy, modify, merge, publish, distribute, sublicense,
 * and/or sell copies of the Software, and to permit persons to whom the
 * Software is furnished to do so, subject to the following conditions:
 *
 * The above copyright notice and this permission notice (including the next
 * paragraph) shall be included in all copies or substantial portions of the
 * Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.  IN NO EVENT SHALL
 * THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
 * LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
 * FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
 * DEALINGS IN THE SOFTWARE.
 */

#include "config.h"

#include <stdlib.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>
#include <linux/input.h>
#include <assert.h>
#include <signal.h>
#include <math.h>
#include <sys/types.h>

#include "shell.h"
#include "frontend/weston.h"
#include <libweston/config-parser.h>
#include "shared/helpers.h"
#include "shared/timespec-util.h"
#include <libweston/shell-utils.h>
#include <libweston/desktop.h>

#define DEFAULT_NUM_WORKSPACES 1

struct focus_state {
	struct desktop_shell *shell;
	struct weston_seat *seat;
	struct workspace *ws;
	struct weston_surface *keyboard_focus;
	struct wl_list link;
	struct wl_listener seat_destroy_listener;
	struct wl_listener surface_destroy_listener;
};

/*
 * Surface stacking and ordering.
 *
 * This is handled using several linked lists of surfaces, organised into
 * ‘layers’. The layers are ordered, and each of the surfaces in one layer are
 * above all of the surfaces in the layer below. The set of layers is static and
 * in the following order (top-most first):
 *  • Cursor layer
 *  • Fullscreen layer
 *  • Workspace layer
 *  • Background layer
 *
 * The list of layers may be manipulated to remove whole layers of surfaces
 * from display.
 *
 * In order to allow popup and transient surfaces to be correctly stacked above
 * their parent surfaces, each surface tracks both its parent surface, and a
 * linked list of its children. When a surface’s layer is updated, so are the
 * layers of its children. Note that child surfaces are *not* the same as
 * subsurfaces — child/parent surfaces are purely for maintaining stacking
 * order.
 *
 * The children_link list of siblings of a surface (i.e. those surfaces which
 * have the same parent) only contains weston_surfaces which have a
 * shell_surface. Stacking is not implemented for non-shell_surface
 * weston_surfaces. This means that the following implication does *not* hold:
 *     (shsurf->parent != NULL) ⇒ !wl_list_is_empty(shsurf->children_link)
 */

struct shell_surface {
	struct wl_signal destroy_signal;

	struct weston_desktop_surface *desktop_surface;
	struct weston_view *view;
	int32_t last_width, last_height;

	struct desktop_shell *shell;

	struct wl_list children_list;
	struct wl_list children_link;

	int unresponsive, grabbed;
	uint32_t resize_edges;

	struct weston_output *output;
	struct wl_listener output_destroy_listener;

	struct {
		bool is_set;
		struct weston_coord_global pos;
	} xwayland;

	int focus_count;

	bool destroying;
	struct wl_list link;	/** desktop_shell::shsurf_list */
};

struct shell_grab {
	struct weston_pointer_grab grab;
	struct shell_surface *shsurf;
	struct wl_listener shsurf_destroy_listener;
};

struct shell_touch_grab {
	struct weston_touch_grab grab;
	struct shell_surface *shsurf;
	struct wl_listener shsurf_destroy_listener;
	struct weston_touch *touch;
};

struct weston_move_grab {
	struct shell_grab base;
	struct weston_coord_global delta;
	bool client_initiated;
};

struct weston_touch_move_grab {
	struct shell_touch_grab base;
	int active;
	struct weston_coord_global delta;
};

struct shell_seat {
	struct weston_seat *seat;
	struct wl_listener seat_destroy_listener;
	struct weston_surface *focused_surface;

	struct wl_listener caps_changed_listener;
	struct wl_listener pointer_focus_listener;
	struct wl_listener keyboard_focus_listener;

	struct wl_list link;	/** shell::seat_list */
};

static void
set_busy_cursor(struct shell_surface *shsurf, struct weston_pointer *pointer);

static struct shell_seat *
get_shell_seat(struct weston_seat *seat);

static void
shell_surface_update_child_surface_layers(struct shell_surface *shsurf);

static struct shell_output *
find_shell_output_from_weston_output(struct desktop_shell *shell,
				     struct weston_output *output)
{
	struct shell_output *shell_output;

	wl_list_for_each(shell_output, &shell->output_list, link) {
		if (shell_output->output == output)
			return shell_output;
	}

	return NULL;
}

static void
destroy_shell_grab_shsurf(struct wl_listener *listener, void *data)
{
	struct shell_grab *grab;

	grab = container_of(listener, struct shell_grab,
			    shsurf_destroy_listener);

	grab->shsurf = NULL;
}

struct weston_view *
get_default_view(struct weston_surface *surface)
{
	struct shell_surface *shsurf;
	struct weston_view *view;

	if (!surface || wl_list_empty(&surface->views))
		return NULL;

	shsurf = get_shell_surface(surface);
	if (shsurf)
		return shsurf->view;

	wl_list_for_each(view, &surface->views, surface_link)
		if (weston_view_is_mapped(view))
			return view;

	return container_of(surface->views.next, struct weston_view, surface_link);
}

static void
desktop_shell_destroy_surface(struct shell_surface *shsurf)
{
	struct shell_surface *shsurf_child, *tmp;

	wl_list_for_each_safe(shsurf_child, tmp, &shsurf->children_list, children_link) {
		wl_list_remove(&shsurf_child->children_link);
		wl_list_init(&shsurf_child->children_link);
	}
	wl_list_remove(&shsurf->children_link);
	weston_desktop_surface_unlink_view(shsurf->view);
	wl_list_remove(&shsurf->link);
	weston_view_destroy(shsurf->view);

	wl_signal_emit(&shsurf->destroy_signal, shsurf);

	if (shsurf->output_destroy_listener.notify) {
		wl_list_remove(&shsurf->output_destroy_listener.link);
		shsurf->output_destroy_listener.notify = NULL;
	}

	free(shsurf);
}

static void
shell_grab_start(struct shell_grab *grab,
		 const struct weston_pointer_grab_interface *interface,
		 struct shell_surface *shsurf,
		 struct weston_pointer *pointer)
{
	weston_seat_break_desktop_grabs(pointer->seat);

	grab->grab.interface = interface;
	grab->shsurf = shsurf;
	grab->shsurf_destroy_listener.notify = destroy_shell_grab_shsurf;
	wl_signal_add(&shsurf->destroy_signal,
		      &grab->shsurf_destroy_listener);

	shsurf->grabbed = 1;
	weston_pointer_start_grab(pointer, &grab->grab);
}

void
get_output_work_area(struct desktop_shell *shell,
		     struct weston_output *output,
		     pixman_rectangle32_t *area)
{
	struct shell_output *sh_output;

	area->x = 0;
	area->y = 0;
	area->width = 0;
	area->height = 0;

	if (!output)
		return;

	sh_output = find_shell_output_from_weston_output(shell, output);
	assert(sh_output);

	area->x = output->pos.c.x;
	area->y = output->pos.c.y;
	area->width = output->width;
	area->height = output->height;

}

static void
shell_grab_end(struct shell_grab *grab)
{
	if (grab->shsurf) {
		wl_list_remove(&grab->shsurf_destroy_listener.link);
		grab->shsurf->grabbed = 0;

		if (grab->shsurf->resize_edges) {
			grab->shsurf->resize_edges = 0;
		}
	}

	weston_pointer_end_grab(grab->grab.pointer);
}

static void
shell_touch_grab_start(struct shell_touch_grab *grab,
		       const struct weston_touch_grab_interface *interface,
		       struct shell_surface *shsurf,
		       struct weston_touch *touch)
{
	weston_seat_break_desktop_grabs(touch->seat);

	grab->grab.interface = interface;
	grab->shsurf = shsurf;
	grab->shsurf_destroy_listener.notify = destroy_shell_grab_shsurf;
	wl_signal_add(&shsurf->destroy_signal,
		      &grab->shsurf_destroy_listener);

	grab->touch = touch;
	shsurf->grabbed = 1;

	weston_touch_start_grab(touch, &grab->grab);
}

static void
shell_touch_grab_end(struct shell_touch_grab *grab)
{
	if (grab->shsurf) {
		wl_list_remove(&grab->shsurf_destroy_listener.link);
		grab->shsurf->grabbed = 0;
	}

	weston_touch_end_grab(grab->touch);
}


static bool
shell_configuration(struct desktop_shell *shell)
{
	struct weston_config_section *section;
	struct weston_config *config;

	config = wet_get_config(shell->compositor);
	section = weston_config_get_section(config, "shell", NULL, NULL);
	weston_config_section_get_color(section, "background-color",
					&shell->background_color, 0xff002244);

	return true;
}








static void
focus_state_destroy(struct focus_state *state)
{
	wl_list_remove(&state->seat_destroy_listener.link);
	wl_list_remove(&state->surface_destroy_listener.link);
	free(state);
}

static void
focus_state_seat_destroy(struct wl_listener *listener, void *data)
{
	struct focus_state *state = container_of(listener,
						 struct focus_state,
						 seat_destroy_listener);

	wl_list_remove(&state->link);
	focus_state_destroy(state);
}

static void
focus_state_surface_destroy(struct wl_listener *listener, void *data)
{
	struct focus_state *state = container_of(listener,
						 struct focus_state,
						 surface_destroy_listener);
	struct weston_surface *main_surface;
	struct weston_view *next;
	struct weston_view *view;

	main_surface = weston_surface_get_main_surface(state->keyboard_focus);

	next = NULL;
	wl_list_for_each(view,
			 &state->ws->layer.view_list.link, layer_link.link) {
		if (view->surface == main_surface)
			continue;
		if (!get_shell_surface(view->surface))
			continue;

		next = view;
		break;
	}

	/* if the focus was a sub-surface, activate its main surface */
	if (main_surface != state->keyboard_focus)
		next = get_default_view(main_surface);

	if (next) {
		if (state->keyboard_focus) {
			wl_list_remove(&state->surface_destroy_listener.link);
			wl_list_init(&state->surface_destroy_listener.link);
		}
		state->keyboard_focus = NULL;
		activate(state->shell, next, state->seat,
			 WESTON_ACTIVATE_FLAG_CONFIGURE);
	} else {
		wl_list_remove(&state->link);
		focus_state_destroy(state);
	}
}

static struct focus_state *
focus_state_create(struct desktop_shell *shell, struct weston_seat *seat,
		   struct workspace *ws)
{
	struct focus_state *state;

	state = malloc(sizeof *state);
	if (state == NULL)
		return NULL;

	state->shell = shell;
	state->keyboard_focus = NULL;
	state->ws = ws;
	state->seat = seat;
	wl_list_insert(&ws->focus_list, &state->link);

	state->seat_destroy_listener.notify = focus_state_seat_destroy;
	state->surface_destroy_listener.notify = focus_state_surface_destroy;
	wl_signal_add(&seat->destroy_signal,
		      &state->seat_destroy_listener);
	wl_list_init(&state->surface_destroy_listener.link);

	return state;
}

static struct focus_state *
ensure_focus_state(struct desktop_shell *shell, struct weston_seat *seat)
{
	struct workspace *ws = get_current_workspace(shell);
	struct focus_state *state;

	wl_list_for_each(state, &ws->focus_list, link)
		if (state->seat == seat)
			break;

	if (&state->link == &ws->focus_list)
		state = focus_state_create(shell, seat, ws);

	return state;
}

static void
focus_state_set_focus(struct focus_state *state,
		      struct weston_surface *surface)
{
	if (state->keyboard_focus) {
		wl_list_remove(&state->surface_destroy_listener.link);
		wl_list_init(&state->surface_destroy_listener.link);
	}

	state->keyboard_focus = surface;
	if (surface)
		wl_signal_add(&surface->destroy_signal,
			      &state->surface_destroy_listener);
}


static void
drop_focus_state(struct desktop_shell *shell, struct workspace *ws,
		 struct weston_surface *surface)
{
	struct focus_state *state;

	wl_list_for_each(state, &ws->focus_list, link)
		if (state->keyboard_focus == surface)
			focus_state_set_focus(state, NULL);
}

static void
desktop_shell_destroy_layer(struct weston_layer *layer);

static void
workspace_destroy(struct workspace *ws)
{
	struct focus_state *state, *next;

	wl_list_for_each_safe(state, next, &ws->focus_list, link)
		focus_state_destroy(state);

	desktop_shell_destroy_layer(&ws->layer);
}

static void
seat_destroyed(struct wl_listener *listener, void *data)
{
	struct weston_seat *seat = data;
	struct focus_state *state, *next;
	struct workspace *ws = container_of(listener,
					    struct workspace,
					    seat_destroyed_listener);

	wl_list_for_each_safe(state, next, &ws->focus_list, link)
		if (state->seat == seat)
			wl_list_remove(&state->link);
}

static void
workspace_create(struct desktop_shell *shell)
{
	struct workspace *ws = &shell->workspace;

	weston_layer_init(&ws->layer, shell->compositor);
	weston_layer_set_position(&ws->layer, WESTON_LAYER_POSITION_NORMAL);

	wl_list_init(&ws->focus_list);
	wl_list_init(&ws->seat_destroyed_listener.link);
	ws->seat_destroyed_listener.notify = seat_destroyed;
}

struct workspace *
get_current_workspace(struct desktop_shell *shell)
{
	return &shell->workspace;
}

static void
surface_keyboard_focus_lost(struct weston_surface *surface)
{
	struct weston_compositor *compositor = surface->compositor;
	struct weston_seat *seat;
	struct weston_surface *focus;

	wl_list_for_each(seat, &compositor->seat_list, link) {
		struct weston_keyboard *keyboard =
			weston_seat_get_keyboard(seat);

		if (!keyboard)
			continue;

		focus = weston_surface_get_main_surface(keyboard->focus);
		if (focus == surface)
			weston_keyboard_set_focus(keyboard, NULL);
	}
}

static void
touch_move_grab_down(struct weston_touch_grab *grab,
		     const struct timespec *time,
		     int touch_id, struct weston_coord_global c)
{
}

static void
touch_move_grab_up(struct weston_touch_grab *grab, const struct timespec *time,
		   int touch_id)
{
	struct weston_touch_move_grab *move =
		(struct weston_touch_move_grab *) container_of(
			grab, struct shell_touch_grab, grab);

	if (touch_id == 0)
		move->active = 0;

	if (grab->touch->num_tp == 0) {
		shell_touch_grab_end(&move->base);
		free(move);
	}
}

static void
touch_move_grab_motion(struct weston_touch_grab *grab,
		       const struct timespec *time, int touch_id,
		       struct weston_coord_global unused)
{
	struct weston_touch_move_grab *move = (struct weston_touch_move_grab *) grab;
	struct shell_surface *shsurf = move->base.shsurf;
	struct weston_coord_global pos;

	if (!shsurf || !shsurf->desktop_surface || !move->active)
		return;

	pos = weston_coord_global_add(grab->touch->grab_pos, move->delta);
	pos.c = weston_coord_truncate(pos.c);
	weston_view_set_position(shsurf->view, pos);
}

static void
touch_move_grab_frame(struct weston_touch_grab *grab)
{
}

static void
touch_move_grab_cancel(struct weston_touch_grab *grab)
{
	struct weston_touch_move_grab *move =
		(struct weston_touch_move_grab *) container_of(
			grab, struct shell_touch_grab, grab);

	shell_touch_grab_end(&move->base);
	free(move);
}

static const struct weston_touch_grab_interface touch_move_grab_interface = {
	touch_move_grab_down,
	touch_move_grab_up,
	touch_move_grab_motion,
	touch_move_grab_frame,
	touch_move_grab_cancel,
};

static int
surface_touch_move(struct shell_surface *shsurf, struct weston_touch *touch)
{
	struct weston_touch_move_grab *move;

	if (!shsurf)
		return -1;


	move = malloc(sizeof *move);
	if (!move)
		return -1;

	move->active = 1;
	move->delta = weston_coord_global_sub(
		weston_view_get_pos_offset_global(shsurf->view),
		touch->grab_pos);

	shell_touch_grab_start(&move->base, &touch_move_grab_interface, shsurf,
			       touch);

	return 0;
}

static void
noop_grab_focus(struct weston_pointer_grab *grab)
{
}

static void
noop_grab_axis(struct weston_pointer_grab *grab,
	       const struct timespec *time,
	       struct weston_pointer_axis_event *event)
{
}

static void
noop_grab_axis_source(struct weston_pointer_grab *grab,
		      uint32_t source)
{
}

static void
noop_grab_frame(struct weston_pointer_grab *grab)
{
}

static struct weston_coord_global
constrain_position(struct weston_move_grab *move)
{
	struct weston_pointer *pointer = move->base.grab.pointer;

	return weston_coord_global_add(pointer->pos, move->delta);
}

static void
move_grab_motion(struct weston_pointer_grab *grab,
		 const struct timespec *time,
		 struct weston_pointer_motion_event *event)
{
	struct weston_move_grab *move = (struct weston_move_grab *) grab;
	struct weston_pointer *pointer = grab->pointer;
	struct shell_surface *shsurf = move->base.shsurf;
	struct weston_coord_global pos;

	weston_pointer_move(pointer, event);
	if (!shsurf || !shsurf->desktop_surface)
		return;

	pos = constrain_position(move);
	weston_view_set_position(shsurf->view, pos);
}

static void
move_grab_button(struct weston_pointer_grab *grab,
		 const struct timespec *time, uint32_t button, uint32_t state_w)
{
	struct shell_grab *shell_grab = container_of(grab, struct shell_grab,
						    grab);
	struct weston_pointer *pointer = grab->pointer;
	enum wl_pointer_button_state state = state_w;

	if (pointer->button_count == 0 &&
	    state == WL_POINTER_BUTTON_STATE_RELEASED) {
		shell_grab_end(shell_grab);
		free(grab);
	}
}

static void
move_grab_cancel(struct weston_pointer_grab *grab)
{
	struct shell_grab *shell_grab =
		container_of(grab, struct shell_grab, grab);

	shell_grab_end(shell_grab);
	free(grab);
}

static const struct weston_pointer_grab_interface move_grab_interface = {
	noop_grab_focus,
	move_grab_motion,
	move_grab_button,
	noop_grab_axis,
	noop_grab_axis_source,
	noop_grab_frame,
	move_grab_cancel,
};

static int
surface_move(struct shell_surface *shsurf, struct weston_pointer *pointer,
	     bool client_initiated)
{
	struct weston_move_grab *move;

	if (!shsurf)
		return -1;

	if (shsurf->grabbed)
		return 0;

	move = malloc(sizeof *move);
	if (!move)
		return -1;

	move->delta = weston_coord_global_sub(
		weston_view_get_pos_offset_global(shsurf->view),
		pointer->grab_pos);
	move->client_initiated = client_initiated;

	shell_grab_start(&move->base, &move_grab_interface, shsurf,
			 pointer);

	return 0;
}

struct weston_resize_grab {
	struct shell_grab base;
	uint32_t edges;
	int32_t width, height;
};



static void
resize_grab_motion(struct weston_pointer_grab *grab,
		   const struct timespec *time,
		   struct weston_pointer_motion_event *event)
{
	struct weston_resize_grab *resize = (struct weston_resize_grab *) grab;
	struct weston_pointer *pointer = grab->pointer;
	struct shell_surface *shsurf = resize->base.shsurf;
	int32_t width, height;
	struct weston_size min_size, max_size;
	struct weston_coord_surface tmp_s;
	wl_fixed_t from_x, from_y;
	wl_fixed_t to_x, to_y;

	weston_pointer_move(pointer, event);

	if (!shsurf || !shsurf->desktop_surface)
		return;

	weston_view_update_transform(shsurf->view);

	tmp_s = weston_coord_global_to_surface(shsurf->view, pointer->grab_pos);
	from_x = wl_fixed_from_double(tmp_s.c.x);
	from_y = wl_fixed_from_double(tmp_s.c.y);
	tmp_s = weston_coord_global_to_surface(shsurf->view, pointer->pos);
	to_x = wl_fixed_from_double(tmp_s.c.x);
	to_y = wl_fixed_from_double(tmp_s.c.y);

	width = resize->width;
	if (resize->edges & WESTON_DESKTOP_SURFACE_EDGE_LEFT) {
		width += wl_fixed_to_int(from_x - to_x);
	} else if (resize->edges & WESTON_DESKTOP_SURFACE_EDGE_RIGHT) {
		width += wl_fixed_to_int(to_x - from_x);
	}

	height = resize->height;
	if (resize->edges & WESTON_DESKTOP_SURFACE_EDGE_TOP) {
		height += wl_fixed_to_int(from_y - to_y);
	} else if (resize->edges & WESTON_DESKTOP_SURFACE_EDGE_BOTTOM) {
		height += wl_fixed_to_int(to_y - from_y);
	}

	max_size = weston_desktop_surface_get_max_size(shsurf->desktop_surface);
	min_size = weston_desktop_surface_get_min_size(shsurf->desktop_surface);

	min_size.width = MAX(1, min_size.width);
	min_size.height = MAX(1, min_size.height);

	if (width < min_size.width)
		width = min_size.width;
	else if (max_size.width > 0 && width > max_size.width)
		width = max_size.width;
	if (height < min_size.height)
		height = min_size.height;
	else if (max_size.height > 0 && height > max_size.height)
		height = max_size.height;
	weston_desktop_surface_set_size(shsurf->desktop_surface, width, height);
}

static void
resize_grab_button(struct weston_pointer_grab *grab,
		   const struct timespec *time,
		   uint32_t button, uint32_t state_w)
{
	struct weston_resize_grab *resize = (struct weston_resize_grab *) grab;
	struct weston_pointer *pointer = grab->pointer;
	enum wl_pointer_button_state state = state_w;

	if (pointer->button_count == 0 &&
	    state == WL_POINTER_BUTTON_STATE_RELEASED) {
		if (resize->base.shsurf && resize->base.shsurf->desktop_surface) {
			struct weston_desktop_surface *desktop_surface =
				resize->base.shsurf->desktop_surface;
			weston_desktop_surface_set_resizing(desktop_surface,
							    false);
			weston_desktop_surface_set_size(desktop_surface, 0, 0);
		}

		shell_grab_end(&resize->base);
		free(grab);
	}
}

static void
resize_grab_cancel(struct weston_pointer_grab *grab)
{
	struct weston_resize_grab *resize = (struct weston_resize_grab *) grab;

	if (resize->base.shsurf && resize->base.shsurf->desktop_surface) {
		struct weston_desktop_surface *desktop_surface =
			resize->base.shsurf->desktop_surface;
		weston_desktop_surface_set_resizing(desktop_surface, false);
		weston_desktop_surface_set_size(desktop_surface, 0, 0);
	}

	shell_grab_end(&resize->base);
	free(grab);
}

static const struct weston_pointer_grab_interface resize_grab_interface = {
	noop_grab_focus,
	resize_grab_motion,
	resize_grab_button,
	noop_grab_axis,
	noop_grab_axis_source,
	noop_grab_frame,
	resize_grab_cancel,
};


static int
surface_resize(struct shell_surface *shsurf,
	       struct weston_pointer *pointer, uint32_t edges)
{
	struct weston_resize_grab *resize;
	const unsigned resize_topbottom =
		WESTON_DESKTOP_SURFACE_EDGE_TOP | WESTON_DESKTOP_SURFACE_EDGE_BOTTOM;
	const unsigned resize_leftright =
		WESTON_DESKTOP_SURFACE_EDGE_LEFT | WESTON_DESKTOP_SURFACE_EDGE_RIGHT;
	const unsigned resize_any = resize_topbottom | resize_leftright;
	struct weston_geometry geometry;

	if (shsurf->grabbed)
		return 0;

	/* Check for invalid edge combinations. */
	if (edges == WESTON_DESKTOP_SURFACE_EDGE_NONE || edges > resize_any ||
	    (edges & resize_topbottom) == resize_topbottom ||
	    (edges & resize_leftright) == resize_leftright)
		return 0;

	resize = malloc(sizeof *resize);
	if (!resize)
		return -1;

	resize->edges = edges;

	geometry = weston_desktop_surface_get_geometry(shsurf->desktop_surface);
	resize->width = geometry.width;
	resize->height = geometry.height;

	shsurf->resize_edges = edges;
	weston_desktop_surface_set_resizing(shsurf->desktop_surface, true);
	shell_grab_start(&resize->base, &resize_grab_interface, shsurf, pointer);

	return 0;
}

static void
busy_cursor_grab_focus(struct weston_pointer_grab *base)
{
	struct shell_grab *grab = (struct shell_grab *) base;
	struct weston_pointer *pointer = base->pointer;
	struct weston_desktop_surface *desktop_surface;
	struct weston_view *view;

	view = weston_compositor_pick_view(pointer->seat->compositor,
					   pointer->pos);
	desktop_surface = weston_surface_get_desktop_surface(view->surface);

	if (!grab->shsurf || grab->shsurf->desktop_surface != desktop_surface) {
		shell_grab_end(grab);
		free(grab);
	}
}

static void
busy_cursor_grab_motion(struct weston_pointer_grab *grab,
			const struct timespec *time,
			struct weston_pointer_motion_event *event)
{
	weston_pointer_move(grab->pointer, event);
}

static void
busy_cursor_grab_button(struct weston_pointer_grab *base,
			const struct timespec *time,
			uint32_t button, uint32_t state)
{
	struct shell_grab *grab = (struct shell_grab *) base;
	struct shell_surface *shsurf = grab->shsurf;
	struct weston_pointer *pointer = grab->grab.pointer;
	struct weston_seat *seat = pointer->seat;

	if (shsurf && button == BTN_LEFT && state)
		activate(shsurf->shell, shsurf->view, seat,
			 WESTON_ACTIVATE_FLAG_CONFIGURE);
}

static void
busy_cursor_grab_cancel(struct weston_pointer_grab *base)
{
	struct shell_grab *grab = (struct shell_grab *) base;

	shell_grab_end(grab);
	free(grab);
}

static const struct weston_pointer_grab_interface busy_cursor_grab_interface = {
	busy_cursor_grab_focus,
	busy_cursor_grab_motion,
	busy_cursor_grab_button,
	noop_grab_axis,
	noop_grab_axis_source,
	noop_grab_frame,
	busy_cursor_grab_cancel,
};

static void
handle_pointer_focus(struct wl_listener *listener, void *data)
{
	struct weston_pointer *pointer = data;
	struct weston_view *view = pointer->focus;
	struct shell_surface *shsurf;
	struct weston_desktop_client *client;

	if (!view)
		return;

	shsurf = get_shell_surface(view->surface);
	if (!shsurf)
		return;

	client = weston_desktop_surface_get_client(shsurf->desktop_surface);

	if (shsurf->unresponsive)
		set_busy_cursor(shsurf, pointer);
	else
		weston_desktop_client_ping(client);
}

static void
has_keyboard_focused_child_callback(struct weston_desktop_surface *surface,
				    void *user_data);

static void
has_keyboard_focused_child_callback(struct weston_desktop_surface *surface,
				    void *user_data)
{
	struct weston_surface *es = weston_desktop_surface_get_surface(surface);
	struct shell_surface *shsurf = get_shell_surface(es);
	bool *has_keyboard_focus = user_data;

	if (shsurf->focus_count > 0) {
		*has_keyboard_focus = true;
		return;
	}

	weston_desktop_surface_foreach_child(shsurf->desktop_surface,
					     has_keyboard_focused_child_callback,
					     &has_keyboard_focus);
}

static bool
has_keyboard_focused_child(struct shell_surface *shsurf)
{
	bool has_keyboard_focus = false;

	if (shsurf->focus_count > 0)
		return true;

	weston_desktop_surface_foreach_child(shsurf->desktop_surface,
					     has_keyboard_focused_child_callback,
					     &has_keyboard_focus);

	return has_keyboard_focus;
}

static void
sync_surface_activated_state(struct shell_surface *shsurf)
{
	struct weston_desktop_surface *surface = shsurf->desktop_surface;
	struct weston_desktop_surface *parent;
	struct weston_surface *parent_surface;

	parent = weston_desktop_surface_get_parent(surface);
	if (parent) {
		parent_surface = weston_desktop_surface_get_surface(parent);
		sync_surface_activated_state(get_shell_surface(parent_surface));
		return;
	}

	if (has_keyboard_focused_child(shsurf))
		weston_desktop_surface_set_activated(surface, true);
	else
		weston_desktop_surface_set_activated(surface, false);
}

static void
shell_surface_deactivate(struct shell_surface *shsurf)
{
	if (--shsurf->focus_count == 0)
		sync_surface_activated_state(shsurf);
}

static void
shell_surface_activate(struct shell_surface *shsurf)
{
	if (shsurf->focus_count++ == 0)
		sync_surface_activated_state(shsurf);
}

/* The surface will be inserted into the list immediately after the link
 * returned by this function (i.e. will be stacked immediately above the
 * returned link). */
static struct weston_layer_entry *
shell_surface_calculate_layer_link (struct shell_surface *shsurf)
{
	struct workspace *ws;

	/* Move the surface to a normal workspace layer so that surfaces
	 * which were previously transient are no longer rendered on top. */
	ws = get_current_workspace(shsurf->shell);
	return &ws->layer.view_list;
}

static void
shell_surface_update_child_surface_layers (struct shell_surface *shsurf)
{
	weston_desktop_surface_propagate_layer(shsurf->desktop_surface);
}

/* Update the surface’s layer. Mark both the old and new views as having dirty
 * geometry to ensure the changes are redrawn.
 *
 * If any child surfaces exist and are mapped, ensure they’re in the same layer
 * as this surface. */
static void
shell_surface_update_layer(struct shell_surface *shsurf)
{
	struct weston_layer_entry *new_layer_link;

	new_layer_link = shell_surface_calculate_layer_link(shsurf);
	assert(new_layer_link);

	weston_view_move_to_layer(shsurf->view, new_layer_link);
	shell_surface_update_child_surface_layers(shsurf);
}

static void
notify_output_destroy(struct wl_listener *listener, void *data)
{
	struct shell_surface *shsurf =
		container_of(listener,
			     struct shell_surface, output_destroy_listener);

	shsurf->output = NULL;
	shsurf->output_destroy_listener.notify = NULL;
}

static void
shell_surface_set_output(struct shell_surface *shsurf,
                         struct weston_output *output)
{
	struct weston_surface *es =
		weston_desktop_surface_get_surface(shsurf->desktop_surface);

	/* get the default output, if the client set it as NULL
	   check whether the output is available */
	if (output)
		shsurf->output = output;
	else if (es->output)
		shsurf->output = es->output;
	else
		shsurf->output = weston_shell_utils_get_default_output(es->compositor);

	if (shsurf->output_destroy_listener.notify) {
		wl_list_remove(&shsurf->output_destroy_listener.link);
		shsurf->output_destroy_listener.notify = NULL;
	}

	if (!shsurf->output)
		return;

	shsurf->output_destroy_listener.notify = notify_output_destroy;
	wl_signal_add(&shsurf->output->destroy_signal,
		      &shsurf->output_destroy_listener);
}

static void
weston_view_set_initial_position(struct weston_view *view,
				 struct desktop_shell *shell);

static void
set_minimized(struct weston_surface *surface)
{
	struct shell_surface *shsurf;
	struct workspace *current_ws;
	struct weston_view *view;

	view = get_default_view(surface);
	if (!view)
		return;

	assert(weston_surface_get_main_surface(view->surface) == view->surface);

	shsurf = get_shell_surface(surface);
	current_ws = get_current_workspace(shsurf->shell);

	weston_view_move_to_layer(view,
				  &shsurf->shell->minimized_layer.view_list);

	drop_focus_state(shsurf->shell, current_ws, view->surface);
	surface_keyboard_focus_lost(surface);

	shell_surface_update_child_surface_layers(shsurf);
}



static void
desktop_shell_destroy_seat(struct shell_seat *shseat)
{

	wl_list_remove(&shseat->keyboard_focus_listener.link);
	wl_list_remove(&shseat->caps_changed_listener.link);
	wl_list_remove(&shseat->pointer_focus_listener.link);
	wl_list_remove(&shseat->seat_destroy_listener.link);

	wl_list_remove(&shseat->link);
	free(shseat);
}

static void
destroy_shell_seat(struct wl_listener *listener, void *data)
{
	struct shell_seat *shseat =
		container_of(listener,
			     struct shell_seat, seat_destroy_listener);

	desktop_shell_destroy_seat(shseat);
}

static void
shell_seat_caps_changed(struct wl_listener *listener, void *data)
{
	struct weston_pointer *pointer;
	struct shell_seat *seat;

	seat = container_of(listener, struct shell_seat, caps_changed_listener);
	pointer = weston_seat_get_pointer(seat->seat);

	if (pointer &&
	    wl_list_empty(&seat->pointer_focus_listener.link)) {
		wl_signal_add(&pointer->focus_signal,
			      &seat->pointer_focus_listener);
	} else if (!pointer) {
		wl_list_remove(&seat->pointer_focus_listener.link);
		wl_list_init(&seat->pointer_focus_listener.link);
	}
}

static struct shell_seat *
create_shell_seat(struct desktop_shell *shell, struct weston_seat *seat)
{
	struct shell_seat *shseat;

	shseat = calloc(1, sizeof *shseat);
	if (!shseat) {
		weston_log("no memory to allocate shell seat\n");
		return NULL;
	}

	shseat->seat = seat;

	shseat->seat_destroy_listener.notify = destroy_shell_seat;
	wl_signal_add(&seat->destroy_signal,
	              &shseat->seat_destroy_listener);

	wl_list_init(&shseat->keyboard_focus_listener.link);

	shseat->pointer_focus_listener.notify = handle_pointer_focus;
	wl_list_init(&shseat->pointer_focus_listener.link);

	shseat->caps_changed_listener.notify = shell_seat_caps_changed;
	wl_signal_add(&seat->updated_caps_signal,
		      &shseat->caps_changed_listener);
	shell_seat_caps_changed(&shseat->caps_changed_listener, NULL);

	wl_list_insert(&shell->seat_list, &shseat->link);

	return shseat;
}

static struct shell_seat *
get_shell_seat(struct weston_seat *seat)
{
	struct wl_listener *listener;

	if (!seat)
		return NULL;

	listener = wl_signal_get(&seat->destroy_signal, destroy_shell_seat);
	if (!listener)
		return NULL;

	return container_of(listener,
			    struct shell_seat, seat_destroy_listener);
}



struct shell_surface *
get_shell_surface(struct weston_surface *surface)
{
	if (weston_surface_is_desktop_surface(surface)) {
		struct weston_desktop_surface *desktop_surface =
			weston_surface_get_desktop_surface(surface);
		return weston_desktop_surface_get_user_data(desktop_surface);
	}
	return NULL;
}

/*
 * libweston-desktop
 */

static void
desktop_surface_added(struct weston_desktop_surface *desktop_surface,
		      void *shell)
{
	struct weston_desktop_client *client =
		weston_desktop_surface_get_client(desktop_surface);
	struct wl_client *wl_client =
		weston_desktop_client_get_client(client);
	struct weston_view *view;
	struct shell_surface *shsurf;
	struct weston_surface *surface =
		weston_desktop_surface_get_surface(desktop_surface);

	view = weston_desktop_surface_create_view(desktop_surface);
	if (!view)
		return;

	shsurf = calloc(1, sizeof *shsurf);
	if (!shsurf) {
		if (wl_client)
			wl_client_post_no_memory(wl_client);
		else
			weston_log("no memory to allocate shell surface\n");
		return;
	}

	weston_surface_set_label_func(surface, weston_shell_utils_surface_get_label);

	shsurf->shell = (struct desktop_shell *) shell;
	shsurf->unresponsive = 0;
	shsurf->desktop_surface = desktop_surface;
	shsurf->view = view;

	shell_surface_set_output(
		shsurf, weston_shell_utils_get_default_output(shsurf->shell->compositor));

	wl_signal_init(&shsurf->destroy_signal);

	/* empty when not in use */

	/*
	 * initialize list as well as link. The latter allows to use
	 * wl_list_remove() even when this surface is not in another list.
	 */
	wl_list_init(&shsurf->children_list);
	wl_list_init(&shsurf->children_link);

	wl_list_insert(&shsurf->shell->shsurf_list, &shsurf->link);

	weston_desktop_surface_set_user_data(desktop_surface, shsurf);
}

static void
desktop_surface_removed(struct weston_desktop_surface *desktop_surface,
			void *shell)
{
	struct shell_surface *shsurf =
		weston_desktop_surface_get_user_data(desktop_surface);
	struct weston_surface *surface =
		weston_desktop_surface_get_surface(desktop_surface);
	struct weston_seat *seat;

	if (!shsurf)
		return;

	wl_list_for_each(seat, &shsurf->shell->compositor->seat_list, link) {
		struct shell_seat *shseat = get_shell_seat(seat);
		/* activate() controls the focused surface activation and
		 * removal of a surface requires invalidating the
		 * focused_surface to avoid activate() use a stale (and just
		 * removed) surface when attempting to de-activate it. It will
		 * also update the focused_surface once it has a chance to run.
		 */
		if (shseat && surface == shseat->focused_surface)
			shseat->focused_surface = NULL;
	}

	weston_surface_set_label_func(surface, NULL);
	weston_desktop_surface_set_user_data(shsurf->desktop_surface, NULL);
	shsurf->desktop_surface = NULL;

	desktop_shell_destroy_surface(shsurf);
}

static void
set_position_from_xwayland(struct shell_surface *shsurf)
{
	struct weston_geometry geometry;
	struct weston_coord_surface offs;

	assert(shsurf->xwayland.is_set);

	geometry = weston_desktop_surface_get_geometry(shsurf->desktop_surface);
	offs = weston_coord_surface(-geometry.x, -geometry.y,
				    shsurf->view->surface);

	weston_view_set_position_with_offset(shsurf->view,
					     shsurf->xwayland.pos,
					     offs);

#ifdef WM_DEBUG
	weston_log("%s: XWM %d, %d; geometry %d, %d; view %f, %f\n",
		   __func__, (int)shsurf->xwayland.pos.c.x, (int)shsurf->xwayland.pos.c.y,
		   (int)geometry.x, (int)geometry.y, pos.c.x, pos.c.y);
#endif
}

static void
map(struct desktop_shell *shell, struct shell_surface *shsurf)
{
	struct weston_surface *surface =
		weston_desktop_surface_get_surface(shsurf->desktop_surface);
	struct weston_compositor *compositor = shell->compositor;
	struct weston_seat *seat;

	/* initial positioning, see also configure() */
	if (shsurf->xwayland.is_set) {
		set_position_from_xwayland(shsurf);
	} else {
		weston_view_set_initial_position(shsurf->view, shell);
	}

	/* XXX: don't map without a buffer! */
	weston_surface_map(surface);

	/* Surface stacking order, see also activate(). */
	shell_surface_update_layer(shsurf);

	wl_list_for_each(seat, &compositor->seat_list, link)
		activate(shell, shsurf->view, seat,
			 WESTON_ACTIVATE_FLAG_CONFIGURE);
}

static void
desktop_surface_committed(struct weston_desktop_surface *desktop_surface,
			  struct weston_coord_surface buf_offset, void *data)
{
	struct shell_surface *shsurf =
		weston_desktop_surface_get_user_data(desktop_surface);
	struct weston_surface *surface =
		weston_desktop_surface_get_surface(desktop_surface);
	struct weston_view *view = shsurf->view;
	struct desktop_shell *shell = data;

	if (surface->width == 0) {
		return;
	}

	if (!weston_surface_is_mapped(surface)) {
		map(shell, shsurf);
		return;
	}

	if (buf_offset.c.x == 0 && buf_offset.c.y == 0 &&
	    shsurf->last_width == surface->width &&
	    shsurf->last_height == surface->height)
	    return;

	weston_view_update_transform(shsurf->view);

	{
		struct weston_coord_surface offset = buf_offset;
		struct weston_coord_global pos;

		if (shsurf->resize_edges) {
			offset.c.x = 0;
			offset.c.y = 0;
		}

		if (shsurf->resize_edges & WESTON_DESKTOP_SURFACE_EDGE_LEFT)
			offset.c.x = shsurf->last_width - surface->width;
		if (shsurf->resize_edges & WESTON_DESKTOP_SURFACE_EDGE_TOP)
			offset.c.y = shsurf->last_height - surface->height;

		pos = weston_view_get_pos_offset_global(view);
		weston_view_set_position_with_offset(shsurf->view, pos, offset);
	}

	shsurf->last_width = surface->width;
	shsurf->last_height = surface->height;

	if (surface->output) {
		wl_list_for_each(view, &surface->views, surface_link)
			weston_view_update_transform(view);
	}
}

static void
desktop_surface_move(struct weston_desktop_surface *desktop_surface,
		     struct weston_seat *seat, uint32_t serial, void *shell)
{
	struct weston_pointer *pointer = weston_seat_get_pointer(seat);
	struct weston_touch *touch = weston_seat_get_touch(seat);
	struct shell_surface *shsurf =
		weston_desktop_surface_get_user_data(desktop_surface);
	struct weston_surface *surface =
		weston_desktop_surface_get_surface(shsurf->desktop_surface);
	struct wl_resource *resource = surface->resource;
	struct weston_surface *focus;

	if (pointer &&
	    pointer->focus &&
	    pointer->button_count > 0 &&
	    pointer->grab_serial == serial) {
		focus = weston_surface_get_main_surface(pointer->focus->surface);
		if ((focus == surface) &&
		    (surface_move(shsurf, pointer, true) < 0))
			wl_resource_post_no_memory(resource);
	} else if (touch &&
		   touch->focus &&
		   touch->grab_serial == serial) {
		focus = weston_surface_get_main_surface(touch->focus->surface);
		if ((focus == surface) &&
		    (surface_touch_move(shsurf, touch) < 0))
			wl_resource_post_no_memory(resource);
	}
}

static void
desktop_surface_resize(struct weston_desktop_surface *desktop_surface,
		       struct weston_seat *seat, uint32_t serial,
		       enum weston_desktop_surface_edge edges, void *shell)
{
	struct weston_pointer *pointer = weston_seat_get_pointer(seat);
	struct shell_surface *shsurf =
		weston_desktop_surface_get_user_data(desktop_surface);
	struct weston_surface *surface =
		weston_desktop_surface_get_surface(shsurf->desktop_surface);
	struct wl_resource *resource = surface->resource;
	struct weston_surface *focus;

	if (!pointer ||
	    pointer->button_count == 0 ||
	    pointer->grab_serial != serial ||
	    pointer->focus == NULL)
		return;

	focus = weston_surface_get_main_surface(pointer->focus->surface);
	if (focus != surface)
		return;

	if (surface_resize(shsurf, pointer, edges) < 0)
		wl_resource_post_no_memory(resource);
}

static void
desktop_surface_set_parent(struct weston_desktop_surface *desktop_surface,
			   struct weston_desktop_surface *parent,
			   void *shell)
{
	struct shell_surface *shsurf_parent;
	struct shell_surface *shsurf =
		weston_desktop_surface_get_user_data(desktop_surface);

	/* unlink any potential child */
	wl_list_remove(&shsurf->children_link);

	if (parent) {
		shsurf_parent = weston_desktop_surface_get_user_data(parent);
		wl_list_insert(shsurf_parent->children_list.prev,
			       &shsurf->children_link);
	} else {
		wl_list_init(&shsurf->children_link);
	}
}

static void
desktop_surface_minimized_requested(struct weston_desktop_surface *desktop_surface,
				    void *shell)
{
	struct weston_surface *surface =
		weston_desktop_surface_get_surface(desktop_surface);

	 /* apply compositor's own minimization logic (hide) */
	set_minimized(surface);
}

static void
set_busy_cursor(struct shell_surface *shsurf, struct weston_pointer *pointer)
{
	struct shell_grab *grab;

	if (pointer->grab->interface == &busy_cursor_grab_interface)
		return;

	grab = malloc(sizeof *grab);
	if (!grab)
		return;

	shell_grab_start(grab, &busy_cursor_grab_interface, shsurf, pointer);
	/* Mark the shsurf as ungrabbed so that button binding is able
	 * to move it. */
	shsurf->grabbed = 0;
}

static void
end_busy_cursor(struct weston_compositor *compositor,
		struct weston_desktop_client *desktop_client)
{
	struct shell_surface *shsurf;
	struct shell_grab *grab;
	struct weston_seat *seat;

	wl_list_for_each(seat, &compositor->seat_list, link) {
		struct weston_pointer *pointer = weston_seat_get_pointer(seat);
		struct weston_desktop_client *grab_client;

		if (!pointer)
			continue;

		if (pointer->grab->interface != &busy_cursor_grab_interface)
			continue;

		grab = (struct shell_grab *) pointer->grab;
		shsurf = grab->shsurf;
		if (!shsurf)
			continue;

		grab_client =
			weston_desktop_surface_get_client(shsurf->desktop_surface);
		if (grab_client  == desktop_client) {
			shell_grab_end(grab);
			free(grab);
		}
	}
}

static void
desktop_surface_set_unresponsive(struct weston_desktop_surface *desktop_surface,
				 void *user_data)
{
	struct shell_surface *shsurf =
		weston_desktop_surface_get_user_data(desktop_surface);
	bool *unresponsive = user_data;

	shsurf->unresponsive = *unresponsive;
}

static void
desktop_surface_ping_timeout(struct weston_desktop_client *desktop_client,
			     void *shell_)
{
	struct desktop_shell *shell = shell_;
	struct shell_surface *shsurf;
	struct weston_seat *seat;
	bool unresponsive = true;

	weston_desktop_client_for_each_surface(desktop_client,
					       desktop_surface_set_unresponsive,
					       &unresponsive);


	wl_list_for_each(seat, &shell->compositor->seat_list, link) {
		struct weston_pointer *pointer = weston_seat_get_pointer(seat);
		struct weston_desktop_client *grab_client;

		if (!pointer || !pointer->focus)
			continue;

		shsurf = get_shell_surface(pointer->focus->surface);
		if (!shsurf)
			continue;

		grab_client =
			weston_desktop_surface_get_client(shsurf->desktop_surface);
		if (grab_client == desktop_client)
			set_busy_cursor(shsurf, pointer);
	}
}

static void
desktop_surface_pong(struct weston_desktop_client *desktop_client,
		     void *shell_)
{
	struct desktop_shell *shell = shell_;
	bool unresponsive = false;

	weston_desktop_client_for_each_surface(desktop_client,
					       desktop_surface_set_unresponsive,
					       &unresponsive);
	end_busy_cursor(shell->compositor, desktop_client);
}

static void
desktop_surface_set_xwayland_position(struct weston_desktop_surface *surface,
				      struct weston_coord_global pos, void *shell_)
{
	struct shell_surface *shsurf =
		weston_desktop_surface_get_user_data(surface);

	shsurf->xwayland.pos = pos;
	shsurf->xwayland.is_set = true;
}

static void
desktop_surface_get_position(struct weston_desktop_surface *surface,
			     int32_t *x, int32_t *y,
			     void *shell_)
{
	struct shell_surface *shsurf = weston_desktop_surface_get_user_data(surface);

	*x = shsurf->view->geometry.pos_offset.x;
	*y = shsurf->view->geometry.pos_offset.y;
}

static const struct weston_desktop_api shell_desktop_api = {
	.struct_size = sizeof(struct weston_desktop_api),
	.surface_added = desktop_surface_added,
	.surface_removed = desktop_surface_removed,
	.committed = desktop_surface_committed,
	.move = desktop_surface_move,
	.resize = desktop_surface_resize,
	.set_parent = desktop_surface_set_parent,
	.minimized_requested = desktop_surface_minimized_requested,
	.ping_timeout = desktop_surface_ping_timeout,
	.pong = desktop_surface_pong,
	.set_xwayland_position = desktop_surface_set_xwayland_position,
	.get_position = desktop_surface_get_position,
};


static struct shell_surface *get_last_child(struct shell_surface *shsurf)
{
	struct shell_surface *shsurf_child;

	wl_list_for_each_reverse(shsurf_child, &shsurf->children_list, children_link) {
		if (weston_view_is_mapped(shsurf_child->view))
			return shsurf_child;
	}

	return NULL;
}

void
activate(struct desktop_shell *shell, struct weston_view *view,
	 struct weston_seat *seat, uint32_t flags)
{
	struct weston_surface *es = view->surface;
	struct weston_surface *main_surface;
	struct focus_state *state;
	struct shell_surface *shsurf, *shsurf_child;
	struct shell_seat *shseat = get_shell_seat(seat);

	main_surface = weston_surface_get_main_surface(es);
	shsurf = get_shell_surface(main_surface);
	assert(shsurf);

	shsurf_child = get_last_child(shsurf);
	if (shsurf_child) {
		/* Activate last xdg child instead of parent. */
		activate(shell, shsurf_child->view, seat, flags);
		return;
	}

	weston_view_activate_input(view, seat, flags);

	if (shseat && shseat->focused_surface &&
	    shseat->focused_surface != main_surface) {
		struct shell_surface *current_focus =
			get_shell_surface(shseat->focused_surface);
		assert(current_focus);
		shell_surface_deactivate(current_focus);
	}

	if (shseat && shseat->focused_surface != main_surface) {
		shell_surface_activate(shsurf);
		shseat->focused_surface = main_surface;
	}

	state = ensure_focus_state(shell, seat);
	if (state == NULL)
		return;

	focus_state_set_focus(state, es);

	/* Update the surface’s layer. This brings it to the top of the stacking
	 * order as appropriate. */
	shell_surface_update_layer(shsurf);
}

static void
activate_binding(struct weston_seat *seat,
		 struct desktop_shell *shell,
		 struct weston_view *focus_view,
		 uint32_t flags)
{
	struct weston_surface *main_surface;

	if (!focus_view)
		return;

	main_surface = weston_surface_get_main_surface(focus_view->surface);
	if (!get_shell_surface(main_surface))
		return;

	activate(shell, focus_view, seat, flags);
}

static void
click_to_activate_binding(struct weston_pointer *pointer,
		          const struct timespec *time,
			  uint32_t button, void *data)
{
	if (pointer->grab != &pointer->default_grab)
		return;
	if (pointer->focus == NULL)
		return;

	activate_binding(pointer->seat, data, pointer->focus,
			 WESTON_ACTIVATE_FLAG_CLICKED |
			 WESTON_ACTIVATE_FLAG_CONFIGURE);
}

static void
touch_to_activate_binding(struct weston_touch *touch,
			  const struct timespec *time,
			  void *data)
{
	if (touch->grab != &touch->default_grab)
		return;
	if (touch->focus == NULL)
		return;

	activate_binding(touch->seat, data, touch->focus,
			 WESTON_ACTIVATE_FLAG_CONFIGURE);
}


static void
transform_handler(struct wl_listener *listener, void *data)
{
	struct weston_surface *surface = data;
	struct shell_surface *shsurf = get_shell_surface(surface);
	const struct weston_xwayland_surface_api *api;
	int x, y;

	if (!shsurf)
		return;

	shell_surface_set_output(shsurf, shsurf->view->output);

	api = shsurf->shell->xwayland_surface_api;
	if (!api) {
		api = weston_xwayland_surface_get_api(shsurf->shell->compositor);
		shsurf->shell->xwayland_surface_api = api;
	}

	if (!api || !api->is_xwayland_surface(surface))
		return;

	if (!weston_view_is_mapped(shsurf->view))
		return;

	x = shsurf->view->geometry.pos_offset.x;
	y = shsurf->view->geometry.pos_offset.y;

	api->send_position(surface, x, y);
}

static void
weston_view_set_initial_position(struct weston_view *view,
				 struct desktop_shell *shell)
{
	struct weston_compositor *compositor = shell->compositor;
	int32_t range_x, range_y;
	int32_t x, y;
	struct weston_output *output, *target_output = NULL;
	struct weston_seat *seat;
	pixman_rectangle32_t area;
	struct weston_coord_global pos;

	/* As a heuristic place the new window on the same output as the
	 * pointer. Falling back to the output containing 0, 0.
	 *
	 * TODO: Do something clever for touch too?
	 */
	pos.c = weston_coord(0, 0);
	wl_list_for_each(seat, &compositor->seat_list, link) {
		struct weston_pointer *pointer = weston_seat_get_pointer(seat);

		if (pointer) {
			pos = pointer->pos;
			break;
		}
	}

	wl_list_for_each(output, &compositor->output_list, link) {
		if (weston_output_contains_coord(output, pos)) {
			target_output = output;
			break;
		}
	}

	if (!target_output) {
		pos.c = weston_coord(10 + random() % 400,
				     10 + random() % 400);
		weston_view_set_position(view, pos);
		return;
	}

	/* Valid range within output where the surface will still be onscreen.
	 * If this is negative it means that the surface is bigger than
	 * output.
	 */
	get_output_work_area(shell, target_output, &area);

	x = area.x;
	y = area.y;
	range_x = area.width - view->surface->width;
	range_y = area.height - view->surface->height;

	if (range_x > 0)
		x += random() % range_x;

	if (range_y > 0)
		y += random() % range_y;

	pos.c = weston_coord(x, y);
	weston_view_set_position(view, pos);
}

static void
shell_reposition_view_on_output_change(struct weston_view *view)
{
	struct weston_output *output, *first_output;
	struct weston_compositor *ec = view->surface->compositor;
	struct shell_surface *shsurf;
	int visible;

	if (wl_list_empty(&ec->output_list))
		return;

	/* At this point the destroyed output is not in the list anymore.
	 * If the view is still visible somewhere, we leave where it is,
	 * otherwise, move it to the first output. */
	visible = 0;
	wl_list_for_each(output, &ec->output_list, link) {
		struct weston_coord_global pos;

		pos = weston_view_get_pos_offset_global(view);
		if (weston_output_contains_coord(output, pos)) {
			visible = 1;
			break;
		}
	}

	shsurf = get_shell_surface(view->surface);
	if (!shsurf)
		return;

	if (!visible) {
		struct weston_coord_global pos;

		first_output = container_of(ec->output_list.next,
					    struct weston_output, link);

		pos = first_output->pos;
		pos.c.x += first_output->width / 4;
		pos.c.y += first_output->height / 4;

		weston_view_set_position(view, pos);
	} else {
		weston_view_geometry_dirty(view);
	}
}

void
shell_for_each_layer(struct desktop_shell *shell,
		     shell_for_each_layer_func_t func, void *data)
{
	func(shell, &shell->background_layer, data);
	func(shell, &shell->workspace.layer, data);
}

static void
shell_output_changed_move_layer(struct desktop_shell *shell,
				struct weston_layer *layer,
				void *data)
{
	struct weston_view *view;

	wl_list_for_each(view, &layer->view_list.link, layer_link.link)
		shell_reposition_view_on_output_change(view);

}

static void
shell_output_destroy(struct shell_output *shell_output)
{
	struct desktop_shell *shell = shell_output->shell;

	shell_for_each_layer(shell, shell_output_changed_move_layer, NULL);

	if (shell_output->background_curtain)
		weston_shell_utils_curtain_destroy(shell_output->background_curtain);
	wl_list_remove(&shell_output->destroy_listener.link);
	wl_list_remove(&shell_output->link);
	free(shell_output);
}

static void
handle_output_destroy(struct wl_listener *listener, void *data)
{
	struct shell_output *shell_output =
		container_of(listener, struct shell_output, destroy_listener);

	shell_output_destroy(shell_output);
}


static void
shell_output_recreate_background(struct shell_output *shell_output);

static void
handle_output_resized(struct wl_listener *listener, void *data)
{
	struct desktop_shell *shell =
		container_of(listener, struct desktop_shell, resized_listener);
	struct weston_output *output = (struct weston_output *)data;
	struct shell_output *sh_output = find_shell_output_from_weston_output(shell, output);

	shell_output_recreate_background(sh_output);
}

static int
background_get_label(struct weston_surface *surface, char *buf, size_t len)
{
	return snprintf(buf, len, "solid background for output %s",
			surface->output ? surface->output->name : "NULL");
}

static void
background_committed(struct weston_surface *es,
		     struct weston_coord_surface new_origin)
{
}

static void
shell_output_recreate_background(struct shell_output *shell_output)
{
	struct desktop_shell *shell = shell_output->shell;
	struct weston_output *output = shell_output->output;
	uint32_t color = shell->background_color;
	struct weston_curtain_params params = {
		.r = ((color >> 16) & 0xff) / 255.0,
		.g = ((color >> 8) & 0xff) / 255.0,
		.b = (color & 0xff) / 255.0,
		.a = ((color >> 24) & 0xff) / 255.0,
		.pos = output->pos,
		.width = output->width, .height = output->height,
		.surface_committed = background_committed,
		.get_label = background_get_label,
		.surface_private = shell_output,
		.capture_input = true,
	};

	if (shell_output->background_curtain)
		weston_shell_utils_curtain_destroy(shell_output->background_curtain);

	shell_output->background_curtain =
		weston_shell_utils_curtain_create(shell->compositor, &params);
	weston_view_set_output(shell_output->background_curtain->view, output);
	weston_view_move_to_layer(shell_output->background_curtain->view,
				  &shell->background_layer.view_list);
}

static void
create_shell_output(struct desktop_shell *shell,
					struct weston_output *output)
{
	struct shell_output *shell_output;

	shell_output = zalloc(sizeof *shell_output);
	if (shell_output == NULL)
		return;

	shell_output->output = output;
	shell_output->shell = shell;
	shell_output_recreate_background(shell_output);
	shell_output->destroy_listener.notify = handle_output_destroy;
	wl_signal_add(&output->destroy_signal,
		      &shell_output->destroy_listener);
	wl_list_insert(shell->output_list.prev, &shell_output->link);

	if (wl_list_length(&shell->output_list) == 1)
		shell_for_each_layer(shell,
				     shell_output_changed_move_layer, NULL);
}

static void
handle_output_create(struct wl_listener *listener, void *data)
{
	struct desktop_shell *shell =
		container_of(listener, struct desktop_shell, output_create_listener);
	struct weston_output *output = (struct weston_output *)data;

	create_shell_output(shell, output);
}

static void
handle_output_move_layer(struct desktop_shell *shell,
			 struct weston_layer *layer, void *data)
{
	struct weston_output *output = data;
	struct weston_view *view;

	wl_list_for_each(view, &layer->view_list.link, layer_link.link) {
		struct weston_coord_global pos;
		if (view->output != output)
			continue;

		pos = weston_coord_global_add(
		      weston_view_get_pos_offset_global(view),
		      output->move);
		weston_view_set_position(view, pos);
	}
}

static void
handle_output_move(struct wl_listener *listener, void *data)
{
	struct desktop_shell *shell;

	shell = container_of(listener, struct desktop_shell,
			     output_move_listener);

	shell_for_each_layer(shell, handle_output_move_layer, data);
}

static void
setup_output_destroy_handler(struct weston_compositor *ec,
							struct desktop_shell *shell)
{
	struct weston_output *output;

	wl_list_for_each(output, &ec->output_list, link)
		create_shell_output(shell, output);

	shell->output_create_listener.notify = handle_output_create;
	wl_signal_add(&ec->output_created_signal,
				&shell->output_create_listener);

	shell->output_move_listener.notify = handle_output_move;
	wl_signal_add(&ec->output_moved_signal, &shell->output_move_listener);
}

static void
desktop_shell_destroy_layer(struct weston_layer *layer)
{
	struct weston_view *view;
	bool removed;

	do {
		removed = false;

		/* Note that we do not choose to destroy all other potential
		 * views we find in the layer, but instead we explicitly verify
		 * if the view in question was explicitly created by
		 * desktop-shell, rather than libweston-desktop (in
		 * desktop_surface_added()).
		 *
		 * This is particularly important because libweston-desktop
		 * could create additional views, which are managed implicitly,
		 * but which are still being added to the layer list.
		 *
		 * We avoid using wl_list_for_each_safe() as it can't handle
		 * removal of the next item in the list, so with this approach
		 * we restart the loop as long as we keep removing views from
		 * the list.
		 */
		wl_list_for_each(view, &layer->view_list.link, layer_link.link) {
			struct shell_surface *shsurf =
				get_shell_surface(view->surface);
			if (shsurf) {
				desktop_shell_destroy_surface(shsurf);
				removed = true;
				break;
			}
		}

	} while (removed);

	weston_layer_fini(layer);
}

static void
shell_destroy(struct wl_listener *listener, void *data)
{
	struct desktop_shell *shell =
		container_of(listener, struct desktop_shell, destroy_listener);
	struct shell_output *shell_output, *tmp;
	struct shell_seat *shseat, *shseat_next;

	wl_list_remove(&shell->destroy_listener.link);
	wl_list_remove(&shell->transform_listener.link);

	wl_list_for_each_safe(shell_output, tmp, &shell->output_list, link)
		shell_output_destroy(shell_output);

	wl_list_remove(&shell->output_create_listener.link);
	wl_list_remove(&shell->output_move_listener.link);
	wl_list_remove(&shell->resized_listener.link);
	wl_list_remove(&shell->session_listener.link);

	wl_list_for_each_safe(shseat, shseat_next, &shell->seat_list, link)
		desktop_shell_destroy_seat(shseat);

	weston_desktop_destroy(shell->desktop);

	workspace_destroy(&shell->workspace);

	desktop_shell_destroy_layer(&shell->background_layer);
	desktop_shell_destroy_layer(&shell->minimized_layer);

	free(shell);
}

static void
shell_add_bindings(struct weston_compositor *ec, struct desktop_shell *shell)
{
	weston_compositor_add_button_binding(ec, BTN_LEFT, 0,
					     click_to_activate_binding,
					     shell);
	weston_compositor_add_button_binding(ec, BTN_RIGHT, 0,
					     click_to_activate_binding,
					     shell);
	weston_compositor_add_touch_binding(ec, 0,
					    touch_to_activate_binding,
					    shell);
	weston_install_debug_key_binding(ec, MODIFIER_SUPER);
}

static void
desktop_shell_notify_session(struct wl_listener *listener, void *data)
{
	struct desktop_shell *shell =
		container_of(listener, struct desktop_shell, session_listener);
	struct weston_compositor *compositor = data;
	struct shell_seat *shseat;

	if (!compositor->session_active)
		return;

	wl_list_for_each(shseat, &shell->seat_list, link) {
		if (!shseat)
			 continue;

		if (shseat->focused_surface) {
			struct shell_surface *current_focus =
				get_shell_surface(shseat->focused_surface);

			if (!current_focus)
				continue;

			weston_view_activate_input(current_focus->view,
						   shseat->seat,
						   WESTON_ACTIVATE_FLAG_NONE);
		}
	}
}

static void
handle_seat_created(struct wl_listener *listener, void *data)
{
	struct weston_seat *seat = data;
	struct desktop_shell *shell =
		container_of(listener, struct desktop_shell, seat_create_listener);

	create_shell_seat(shell, seat);
}

WL_EXPORT int
wet_shell_init(struct weston_compositor *ec,
	       int *argc, char *argv[])
{
	struct weston_seat *seat;
	struct desktop_shell *shell;

	shell = zalloc(sizeof *shell);
	if (shell == NULL)
		return -1;

	shell->compositor = ec;

	if (!weston_compositor_add_destroy_listener_once(ec,
							 &shell->destroy_listener,
							 shell_destroy)) {
		free(shell);
		return 0;
	}

	shell->transform_listener.notify = transform_handler;
	wl_signal_add(&ec->transform_signal, &shell->transform_listener);

	weston_layer_init(&shell->background_layer, ec);

	weston_layer_set_position(&shell->background_layer,
				  WESTON_LAYER_POSITION_BACKGROUND);

	wl_list_init(&shell->seat_list);
	wl_list_init(&shell->shsurf_list);
	wl_list_init(&shell->output_list);
	wl_list_init(&shell->output_create_listener.link);
	wl_list_init(&shell->output_move_listener.link);
	wl_list_init(&shell->seat_create_listener.link);
	wl_list_init(&shell->resized_listener.link);
	wl_list_init(&shell->workspace.focus_list);
	wl_list_init(&shell->workspace.seat_destroyed_listener.link);

	weston_layer_init(&shell->minimized_layer, ec);
	weston_layer_init(&shell->workspace.layer, ec);

	if (!shell_configuration(shell))
		return -1;

	workspace_create(shell);

	shell->desktop = weston_desktop_create(ec, &shell_desktop_api, shell);
	if (!shell->desktop)
		return -1;

	setup_output_destroy_handler(ec, shell);

	wl_list_for_each(seat, &ec->seat_list, link)
		create_shell_seat(shell, seat);
	shell->seat_create_listener.notify = handle_seat_created;
	wl_signal_add(&ec->seat_created_signal, &shell->seat_create_listener);

	shell->resized_listener.notify = handle_output_resized;
	wl_signal_add(&ec->output_resized_signal, &shell->resized_listener);

	shell->session_listener.notify = desktop_shell_notify_session;
	wl_signal_add(&ec->session_signal, &shell->session_listener);
	screenshooter_create(ec);

	shell_add_bindings(ec, shell);

	clock_gettime(CLOCK_MONOTONIC, &shell->startup_time);

	return 0;
}
