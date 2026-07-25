/* Fake-C-object test support (D18), compiled only under the
 * `testsupport` cargo feature.  Provides just enough real libwayland
 * machinery — wl_signal emission against real wl_list links — that the
 * `weston` crate's Listener / registry / dispatch primitives are
 * exercisable in `cargo test` without a compositor. */
#ifndef WESTON_SYS_TESTSUPPORT_H
#define WESTON_SYS_TESTSUPPORT_H

struct wl_signal;

/* A heap-allocated wl_signal the test controls. */
struct wl_signal *wsys_test_signal_create(void);
void wsys_test_signal_emit(struct wl_signal *signal, void *data);
void wsys_test_signal_destroy(struct wl_signal *signal);

#endif
