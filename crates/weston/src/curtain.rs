//! Background curtains (kind 2: C allocates on request, we destroy —
//! plan §3a), with the sync-tier `get_label` callback answering from
//! callback-local data only (A3; §3e).

use std::ffi::{c_char, c_int};
use std::ptr::NonNull;

use crate::ctx::Ctx;
use crate::ids::OutputId;

pub(crate) struct Curtain {
    raw: NonNull<weston_sys::weston_curtain>,
}

/// Sync tier, no app borrow: mirrors C `background_get_label`, which
/// formats from the surface's own output pointer — callback-local data.
unsafe extern "C" fn curtain_get_label(
    es: *mut weston_sys::weston_surface,
    buf: *mut c_char,
    len: usize,
) -> c_int {
    crate::panic_barrier::guard("curtain_get_label", || {
        if buf.is_null() || len == 0 {
            return 0;
        }
        // SAFETY: es is the live curtain surface inside this callback;
        // output/name are POD reads valid for the call (§3a kind 5).
        let name = unsafe {
            let out = (*es).output;
            if out.is_null() || (*out).name.is_null() {
                "NULL".to_string()
            } else {
                std::ffi::CStr::from_ptr((*out).name)
                    .to_string_lossy()
                    .into_owned()
            }
        };
        let text = format!("solid background for output {name}");
        let bytes = text.as_bytes();
        let n = bytes.len().min(len - 1);
        // SAFETY: buf has len writable bytes per the C label contract.
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr().cast::<c_char>(), buf, n);
            *buf.add(n) = 0;
        }
        n as c_int
    })
}

unsafe extern "C" fn curtain_committed(
    _es: *mut weston_sys::weston_surface,
    _new_origin: weston_sys::weston_coord_surface,
) {
    // C background_committed is empty.
}

/// (Re)create the solid background for an output, mirroring
/// `shell_output_recreate_background`.
pub(crate) fn recreate_background(ctx: &Ctx, output: OutputId, argb: u32) {
    destroy_background(ctx, output);

    let Some(out) = ctx.resolve_output(output) else {
        return;
    };
    let Some(layer) = crate::layer::background_layer_ptr(ctx) else {
        return;
    };

    // SAFETY: registry-live output; POD reads.
    let (pos, width, height) = unsafe {
        let o = out.as_ref();
        (o.pos, o.width, o.height)
    };

    let mut params = weston_sys::weston_curtain_params {
        get_label: Some(curtain_get_label),
        surface_committed: Some(curtain_committed),
        surface_private: std::ptr::null_mut(),
        r: ((argb >> 16) & 0xff) as f32 / 255.0,
        g: ((argb >> 8) & 0xff) as f32 / 255.0,
        b: (argb & 0xff) as f32 / 255.0,
        a: ((argb >> 24) & 0xff) as f32 / 255.0,
        pos,
        width,
        height,
        capture_input: true,
    };

    // SAFETY: compositor live; params fully initialized and only read
    // during the call; the returned curtain is owned by us (kind 2).
    let curtain = ctx.with_depth(|| unsafe {
        weston_sys::weston_shell_utils_curtain_create(ctx.compositor_ptr(), &mut params)
    });
    let Some(curtain) = NonNull::new(curtain) else {
        return;
    };

    // SAFETY: curtain view live; mirror the C set_output +
    // move_to_layer pair.
    ctx.with_depth(|| unsafe {
        let view = (*curtain.as_ptr()).view;
        weston_sys::weston_view_set_output(view, out.as_ptr());
        weston_sys::weston_view_move_to_layer(view, &raw mut (*layer).view_list);
    });

    ctx.inner
        .curtains
        .borrow_mut()
        .insert(output, Curtain { raw: curtain });
}

/// Destroy the output's curtain if present (output death / recreate).
pub(crate) fn destroy_background(ctx: &Ctx, output: OutputId) {
    let old = ctx.inner.curtains.borrow_mut().remove(&output);
    if let Some(c) = old {
        // SAFETY: we own the curtain (kind 2); single destroy call in
        // the documented order.
        ctx.with_depth(|| unsafe {
            weston_sys::weston_shell_utils_curtain_destroy(c.raw.as_ptr());
        });
    }
}

/// Move a moved output's curtain along with it (C handle_output_move
/// touches every layer view with view->output == moved output; the
/// curtains are the wrapper-owned subset).
pub(crate) fn move_backgrounds_with_output(ctx: &Ctx, output: OutputId, dx: f64, dy: f64) {
    let curtain = ctx.inner.curtains.borrow().get(&output).map(|c| c.raw);
    if let Some(c) = curtain {
        // SAFETY: live curtain view; POD math mirrors
        // handle_output_move_layer.
        ctx.with_depth(|| unsafe {
            let view = (*c.as_ptr()).view;
            let mut pos = weston_sys::weston_view_get_pos_offset_global(view);
            pos.c.x += dx;
            pos.c.y += dy;
            weston_sys::weston_view_set_position(view, pos);
        });
    }
}

pub(crate) fn destroy_all_backgrounds(ctx: &Ctx) {
    let keys: Vec<OutputId> = ctx.inner.curtains.borrow().keys().copied().collect();
    for k in keys {
        destroy_background(ctx, k);
    }
}
