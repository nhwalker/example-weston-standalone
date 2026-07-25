#include "testsupport.h"

#include <stdlib.h>

#include <wayland-server-core.h>

struct wl_signal *
wsys_test_signal_create(void)
{
	struct wl_signal *s = calloc(1, sizeof *s);

	if (s)
		wl_signal_init(s);
	return s;
}

void
wsys_test_signal_emit(struct wl_signal *signal, void *data)
{
	wl_signal_emit(signal, data);
}

void
wsys_test_signal_destroy(struct wl_signal *signal)
{
	free(signal);
}
