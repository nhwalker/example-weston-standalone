//! Shell input bindings (C shell_add_bindings): click/touch to
//! activate + libweston's own debug key binding.  Sync tier — the
//! handlers read the input frame, then hand the shell a sync
//! `Activate` event (inventory rows B3–B6).

use std::ffi::c_void;
use std::ptr::NonNull;

use crate::ctx::Ctx;
use crate::events::{ActivateVia, Event};
use crate::ids::SeatId;
use crate::panic_barrier;

const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;

unsafe extern "C" fn click_to_activate(
    pointer: *mut weston_sys::weston_pointer,
    _time: *const weston_sys::timespec,
    _button: u32,
    _data: *mut c_void,
) {
    panic_barrier::guard("binding.click_to_activate", || {
        let Some(ctx) = Ctx::current() else { return };
        ctx.with_depth(|| {
            // SAFETY: pointer live inside the binding frame; mirrors
            // click_to_activate_binding's guards.
            let (in_default_grab, focus, seat) = unsafe {
                let p = &*pointer;
                (
                    std::ptr::eq(p.grab, &raw const p.default_grab),
                    NonNull::new(p.focus),
                    NonNull::new(p.seat),
                )
            };
            if !in_default_grab {
                return;
            }
            let (Some(focus), Some(seat)) = (focus, seat) else {
                return;
            };
            // SAFETY: focus view live in the input frame.
            let Some(surf) = NonNull::new(unsafe { focus.as_ref().surface }) else {
                return;
            };
            let Some(main) = ctx.main_ds_id_of_wsurf(surf) else {
                return;
            };
            let Some(seat_id) = ctx.inner.seats.borrow().id_of(seat) else {
                return;
            };
            ctx.dispatch_sync(Event::Activate {
                seat: SeatId(seat_id),
                surface: main,
                via: ActivateVia::PointerBinding,
            });
        });
    });
}

unsafe extern "C" fn touch_to_activate(
    touch: *mut weston_sys::weston_touch,
    _time: *const weston_sys::timespec,
    _data: *mut c_void,
) {
    panic_barrier::guard("binding.touch_to_activate", || {
        let Some(ctx) = Ctx::current() else { return };
        ctx.with_depth(|| {
            // SAFETY: touch live inside the binding frame; mirrors
            // touch_to_activate_binding's guards.
            let (in_default_grab, focus, seat) = unsafe {
                let t = &*touch;
                (
                    std::ptr::eq(t.grab, &raw const t.default_grab),
                    NonNull::new(t.focus),
                    NonNull::new(t.seat),
                )
            };
            if !in_default_grab {
                return;
            }
            let (Some(focus), Some(seat)) = (focus, seat) else {
                return;
            };
            // SAFETY: focus view live in the input frame.
            let Some(surf) = NonNull::new(unsafe { focus.as_ref().surface }) else {
                return;
            };
            let Some(main) = ctx.main_ds_id_of_wsurf(surf) else {
                return;
            };
            let Some(seat_id) = ctx.inner.seats.borrow().id_of(seat) else {
                return;
            };
            ctx.dispatch_sync(Event::Activate {
                seat: SeatId(seat_id),
                surface: main,
                via: ActivateVia::TouchBinding,
            });
        });
    });
}

pub(crate) fn add_shell_bindings(ctx: &Ctx) {
    let ec = ctx.compositor_ptr();
    // SAFETY: compositor live; binding registrations mirror
    // shell_add_bindings (BTN_LEFT/BTN_RIGHT/touch + debug key).
    unsafe {
        weston_sys::weston_compositor_add_button_binding(
            ec,
            BTN_LEFT,
            0,
            Some(click_to_activate),
            std::ptr::null_mut(),
        );
        weston_sys::weston_compositor_add_button_binding(
            ec,
            BTN_RIGHT,
            0,
            Some(click_to_activate),
            std::ptr::null_mut(),
        );
        weston_sys::weston_compositor_add_touch_binding(
            ec,
            0,
            Some(touch_to_activate),
            std::ptr::null_mut(),
        );
        weston_sys::weston_install_debug_key_binding(
            ec,
            weston_sys::weston_keyboard_modifier::MODIFIER_SUPER,
        );
    }
}
