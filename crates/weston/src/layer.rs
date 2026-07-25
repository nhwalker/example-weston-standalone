//! Shell layers: kind-4 embedded-by-value C structs (plan §3a).
//!
//! `weston_layer_init` registers an intrusive list node *at the struct's
//! address*, so the storage must never move: pinned boxes, created and
//! destroyed only here.  The safe crates never name the type; the shell
//! sees nothing (stacking commands go through `ShellHost`).

use std::cell::UnsafeCell;
use std::pin::Pin;

use crate::ctx::Ctx;

pub(crate) struct ShellLayers {
    background: Pin<Box<UnsafeCell<weston_sys::weston_layer>>>,
    workspace: Pin<Box<UnsafeCell<weston_sys::weston_layer>>>,
}

fn new_layer(
    ctx: &Ctx,
    position: weston_sys::weston_layer_position::Type,
) -> Pin<Box<UnsafeCell<weston_sys::weston_layer>>> {
    // SAFETY: weston_layer is POD storage for weston_layer_init; the box
    // is pinned before C ever learns the address.
    let boxed = Box::pin(UnsafeCell::new(unsafe {
        std::mem::zeroed::<weston_sys::weston_layer>()
    }));
    // SAFETY: compositor live; the address is stable (pinned) for the
    // lifetime of ShellLayers, released via weston_layer_fini below.
    unsafe {
        weston_sys::weston_layer_init(boxed.get(), ctx.compositor_ptr());
        weston_sys::weston_layer_set_position(boxed.get(), position);
    }
    boxed
}

/// Create both shell layers (background + workspace), mirroring
/// wet_shell_init's weston_layer_init/set_position pairs.
pub(crate) fn create_shell_layers(ctx: &Ctx) {
    let layers = ShellLayers {
        background: new_layer(
            ctx,
            weston_sys::weston_layer_position::WESTON_LAYER_POSITION_BACKGROUND,
        ),
        workspace: new_layer(
            ctx,
            weston_sys::weston_layer_position::WESTON_LAYER_POSITION_NORMAL,
        ),
    };
    *ctx.inner.shell_layers.borrow_mut() = Some(layers);
}

/// Fini both layers (shell teardown; views must already be gone).
pub(crate) fn destroy_shell_layers(ctx: &Ctx) {
    if let Some(layers) = ctx.inner.shell_layers.borrow_mut().take() {
        // SAFETY: layers were init'ed in create_shell_layers; the shell
        // has destroyed its views and curtains are gone by now
        // (teardown ordering mirrors C shell_destroy).
        unsafe {
            weston_sys::weston_layer_fini(layers.workspace.get());
            weston_sys::weston_layer_fini(layers.background.get());
        }
    }
}

pub(crate) fn workspace_layer_ptr(ctx: &Ctx) -> Option<*mut weston_sys::weston_layer> {
    ctx.inner
        .shell_layers
        .borrow()
        .as_ref()
        .map(|l| l.workspace.get())
}

pub(crate) fn background_layer_ptr(ctx: &Ctx) -> Option<*mut weston_sys::weston_layer> {
    ctx.inner
        .shell_layers
        .borrow()
        .as_ref()
        .map(|l| l.background.get())
}
