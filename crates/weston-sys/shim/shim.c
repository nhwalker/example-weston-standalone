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

/* One bounded stack buffer per call; long lines are silently truncated
 * (the sink receives only buf/len/cont -- vsnprintf's return value is
 * not forwarded, so truncation is invisible to it; PR16-S5). */

#define WSYS_LOG_BUF 1024

/* C main.c keeps these two as file statics for the same reason: the
 * weston_log_set_handler signature has no user-data slot. */
static struct weston_log_scope *wsys_log_scope;
static int wsys_cached_tm_mday = -1;

static int
wsys_sink(const char *fmt, va_list ap, bool cont)
{
	char buf[WSYS_LOG_BUF];
	int n = vsnprintf(buf, sizeof buf, fmt, ap);

	if (n < 0)
		return 0;
	return wsys_rust_log_sink(buf, (size_t)(n < WSYS_LOG_BUF ? n : WSYS_LOG_BUF - 1),
				  cont);
}

/* C main.c vlog (214): timestamp the line and print it into the scope.
 * The is_enabled check is C's -- with no subscriber on "log" the line
 * is dropped rather than formatted, which is exactly what makes
 * `--logger-scopes=drm-backend` remove ordinary log output. */
static int
wsys_vlog(const char *fmt, va_list ap)
{
	char timestr[128];
	char buf[WSYS_LOG_BUF];
	int n;

	if (!wsys_log_scope)
		return wsys_sink(fmt, ap, false);

	if (!weston_log_scope_is_enabled(wsys_log_scope))
		return 0;

	n = vsnprintf(buf, sizeof buf, fmt, ap);
	if (n < 0)
		return weston_log_scope_printf(wsys_log_scope, "%s %s",
					       weston_log_timestamp(timestr,
								    sizeof timestr,
								    &wsys_cached_tm_mday),
					       "Out of memory");
	return weston_log_scope_printf(wsys_log_scope, "%s %s",
				       weston_log_timestamp(timestr, sizeof timestr,
							    &wsys_cached_tm_mday),
				       buf);
}

/* C main.c vlog_continue (241): no timestamp, straight into the scope
 * -- this is how multi-line weston_log_continue blocks stay attached to
 * the line above them. */
static int
wsys_vlog_continue(const char *fmt, va_list ap)
{
	if (!wsys_log_scope)
		return wsys_sink(fmt, ap, true);

	return weston_log_scope_vprintf(wsys_log_scope, fmt, ap);
}

void
wsys_install_log_handlers(struct weston_log_scope *scope)
{
	wsys_log_scope = scope;
	weston_log_set_handler(wsys_vlog, wsys_vlog_continue);
}
