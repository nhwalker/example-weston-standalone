/* C shim for weston-sys (plan §3k).
 *
 * Two jobs:
 *  1. Export callable symbols for the static-inline helpers in the
 *     installed headers that the Rust side actually uses (bindgen cannot
 *     bind static inlines).  The set is grown deliberately, one function
 *     at a time, as the port needs them — not bulk-generated.
 *  2. Own the va_list interfaces stable Rust cannot define: the
 *     weston_log_set_handler pair.  The C side formats into a bounded
 *     buffer and hands Rust a length-delimited UTF-8-unchecked byte
 *     string.
 */
#ifndef WESTON_SYS_SHIM_H
#define WESTON_SYS_SHIM_H

#include <stddef.h>
#include <stdbool.h>

struct wl_list;
struct wl_signal;
struct wl_listener;

/* --- wayland-util static inlines ---------------------------------- */

void wsys_wl_list_init(struct wl_list *list);
void wsys_wl_list_remove(struct wl_list *elm);
bool wsys_wl_list_empty(const struct wl_list *list);

/* --- wayland-server-core static inlines --------------------------- */

void wsys_wl_signal_add(struct wl_signal *signal, struct wl_listener *listener);

/* --- weston-log va_list handlers (§3k) ----------------------------- */

/* Rust-side sink, defined in the `weston` crate:
 * receives one formatted log line (not NUL-terminated, len bytes),
 * `cont` distinguishes vlog_continue.  Returns chars "printed" so the
 * C handler can satisfy weston_log's int return. */
extern int wsys_rust_log_sink(const char *buf, size_t len, bool cont);

/* Install wsys-owned vlog/vlog_continue via weston_log_set_handler. */
void wsys_install_log_handlers(void);

#endif
