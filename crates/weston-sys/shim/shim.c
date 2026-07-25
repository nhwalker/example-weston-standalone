/* See shim.h for the contract.  Everything here compiles against the
 * installed EPEL headers only. */
#include "shim.h"

#include <stdarg.h>
#include <stdio.h>

#include <wayland-server-core.h>
#include <libweston/libweston.h>
#include <libweston/weston-log.h>

void
wsys_wl_list_init(struct wl_list *list)
{
	wl_list_init(list);
}

void
wsys_wl_list_remove(struct wl_list *elm)
{
	wl_list_remove(elm);
}

bool
wsys_wl_list_empty(const struct wl_list *list)
{
	return wl_list_empty(list);
}

void
wsys_wl_signal_add(struct wl_signal *signal, struct wl_listener *listener)
{
	wl_signal_add(signal, listener);
}

/* --- weston-log va_list handlers ----------------------------------- */

/* One bounded stack buffer per call; long lines are truncated, which the
 * sink can see from the return value of vsnprintf if it ever matters.
 * The R2a frontend port will replace this basic pair with the scope-aware
 * handlers (flight recorder etc.); this one exists so R0/R1 code and the
 * smoke binary have working weston_log output. */

#define WSYS_LOG_BUF 1024

static int
wsys_vlog(const char *fmt, va_list ap)
{
	char buf[WSYS_LOG_BUF];
	int n = vsnprintf(buf, sizeof buf, fmt, ap);

	if (n < 0)
		return 0;
	return wsys_rust_log_sink(buf, (size_t)(n < WSYS_LOG_BUF ? n : WSYS_LOG_BUF - 1),
				  false);
}

static int
wsys_vlog_continue(const char *fmt, va_list ap)
{
	char buf[WSYS_LOG_BUF];
	int n = vsnprintf(buf, sizeof buf, fmt, ap);

	if (n < 0)
		return 0;
	return wsys_rust_log_sink(buf, (size_t)(n < WSYS_LOG_BUF ? n : WSYS_LOG_BUF - 1),
				  true);
}

void
wsys_install_log_handlers(void)
{
	weston_log_set_handler(wsys_vlog, wsys_vlog_continue);
}
