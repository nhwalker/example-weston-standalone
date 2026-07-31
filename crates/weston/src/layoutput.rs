//! Layout outputs: the DRM head→output grouping (plan §7 R2c).
//!
//! Every other backend takes the *simple* path — one head, one output,
//! configure, enable, done (`simple_heads_changed` in C,
//! `heads_changed` + `enable_head` here).  DRM does not, because on
//! DRM several connectors can be driven as clones of one logical
//! output, and because a mode set can *fail* at enable time in ways
//! only the kernel knows about.
//!
//! So C gives DRM its own handler and a small pile of bookkeeping —
//! `wet_layoutput`, `wet_output`, `wet_head_array`,
//! `drm_process_layoutput(s)`, `drm_try_attach`, `drm_try_enable` —
//! and this module is that pile.  The shape is worth stating once,
//! because it is not obvious from any single function:
//!
//! - A **layoutput** is a named group of heads that will be driven as
//!   clones.  Its name comes from the `[[output]]` section's `name=`
//!   (so two heads whose sections share a name land in one group), or
//!   from the head itself when no section matches.
//! - Heads arriving in a flush are *staged* on the layoutput's `add`
//!   list, not enabled immediately.  C does this because the whole
//!   point is to collect all the clones of one output before creating
//!   it — enabling the first one eagerly would foreclose the group.
//! - Enabling is then **attach-then-enable with undo**: attach the
//!   staged heads, try to enable, and on failure detach the heads one
//!   at a time and retry, until either it comes up or nothing is left.
//!   Heads that lost are pushed to the next round rather than dropped.
//!
//! That retry loop is the reason this cannot collapse into the simple
//! path: `weston_output_enable` failing is normal on real hardware
//! (not enough CRTCs, incompatible modes), and the frontend is
//! expected to degrade rather than abort.

use std::cell::Cell;
use std::ffi::CString;
use std::ptr::NonNull;
use std::rc::Rc;

use crate::ctx::Ctx;
use crate::listener::Listener;
use crate::log;
use crate::output_policy::{DrmMode, DrmOutputSetup, OutputPolicy};

/// C `MAX_CLONE_HEADS`.  C's guard is `if (n + 1 >= ARRAY_LENGTH)`,
/// which caps the list one below the array size; kept exactly, because
/// a group that silently grew past C's limit would diverge from the
/// oracle in the one case hardest to reproduce.
const MAX_CLONE_HEADS: usize = 16;

/// C `wet_output`: one weston_output belonging to a layoutput, plus the
/// destroy listener that nulls the pointer when the output dies.
///
/// The output can be destroyed behind our back — `drm_output_destroy()`
/// defers destruction in some paths — so the pointer is a `Cell` that
/// the listener clears, exactly as C nulls `output->output`.
pub(crate) struct LayoutputOutput {
    /// Shared with the destroy listener's closure — that is the whole
    /// point of the `Rc`: the listener must clear the same cell the
    /// readers look at, or a destroyed output stays live to us.
    output: Rc<Cell<*mut weston_sys::weston_output>>,
    /// Held so the listener lives as long as the entry.
    _destroy: Listener,
}

impl LayoutputOutput {
    fn get(&self) -> Option<NonNull<weston_sys::weston_output>> {
        NonNull::new(self.output.get())
    }
}

/// C `wet_layoutput`.
pub(crate) struct Layoutput {
    /// The logical output name (the section's `name=`, or the head's).
    pub(crate) name: String,
    /// The head whose *controlling* rule configures this group.  C
    /// stores the resolved `weston_config_section *`; the policy here
    /// is keyed by head name, so the name is what we keep.
    pub(crate) section_head: String,
    outputs: Vec<Rc<LayoutputOutput>>,
    /// C `wet_layoutput::add` — heads staged for this flush.  Entries
    /// are cleared (not removed) as they are consumed, because C's
    /// undo walk indexes from the end and relies on the holes.
    add: Vec<*mut weston_sys::weston_head>,
}

impl Layoutput {
    fn new(name: String, section_head: String) -> Layoutput {
        Layoutput {
            name,
            section_head,
            outputs: Vec::new(),
            add: Vec::new(),
        }
    }
}

/// C `wet_compositor_layoutput_add_head` (main.c:2718): find the group
/// by name or create it, then stage the head.
pub(crate) fn add_head(
    ctx: &Ctx,
    output_name: &str,
    section_head: &str,
    head: NonNull<weston_sys::weston_head>,
) {
    let mut los = ctx.inner.layoutputs.borrow_mut();
    let idx = match los.iter().position(|lo| lo.name == output_name) {
        Some(i) => i,
        None => {
            los.push(Layoutput::new(
                output_name.to_string(),
                section_head.to_string(),
            ));
            los.len() - 1
        }
    };
    // C's cap, verbatim: `n + 1 >= ARRAY_LENGTH` drops the head
    // silently once the group is full.
    if los[idx].add.len() + 1 >= MAX_CLONE_HEADS {
        return;
    }
    los[idx].add.push(head.as_ptr());
}

/// C `drm_try_attach` (main.c:2815): attach every staged head after the
/// first (the first came in with the output at creation).  A head that
/// will not attach moves to `failed` and its slot is cleared.
fn try_attach(
    output: NonNull<weston_sys::weston_output>,
    add: &mut [*mut weston_sys::weston_head],
    failed: &mut Vec<*mut weston_sys::weston_head>,
) {
    for slot in add.iter_mut().skip(1) {
        if slot.is_null() {
            continue;
        }
        // SAFETY: output live and not yet enabled; the head pointer came
        // from this flush's snapshot and nothing above has freed it.
        let ret = unsafe { weston_sys::weston_output_attach_head(output.as_ptr(), *slot) };
        if ret < 0 {
            failed.push(*slot);
            *slot = std::ptr::null_mut();
        }
    }
}

/// C `drm_try_enable` (main.c:2836): enable, and on failure detach one
/// head and try again, until it comes up or there is nothing left to
/// give back.
///
/// The walk is from the END of the staged list and skips already-null
/// slots — that is why `add` keeps its holes rather than compacting.
fn try_enable(
    ctx: &Ctx,
    output: NonNull<weston_sys::weston_output>,
    undo: &mut Vec<*mut weston_sys::weston_head>,
    failed: &mut Vec<*mut weston_sys::weston_head>,
) -> bool {
    loop {
        // SAFETY: output live; `enabled` is a bound POD field.
        if unsafe { (*output.as_ptr()).enabled } {
            return true;
        }
        crate::compositor::lazy_align(ctx.inner.compositor.get(), output);
        // SAFETY: output live, configured, not enabled.
        let ok = ctx.with_depth(|| unsafe { weston_sys::weston_output_enable(output.as_ptr()) });
        if ok == 0 {
            return true;
        }

        // The next head to drop: walk back over the holes.
        let mut victim = None;
        while let Some(last) = undo.pop() {
            if !last.is_null() {
                victim = Some(last);
                break;
            }
        }
        // No heads left to undo and still not enabled.
        let Some(victim) = victim else {
            return false;
        };
        // SAFETY: head live and attached to this output.
        unsafe { weston_sys::weston_head_detach(victim) };
        failed.push(victim);
    }
}

/// C `drm_try_attach_enable` (main.c:2867).
fn try_attach_enable(
    ctx: &Ctx,
    output: NonNull<weston_sys::weston_output>,
    lo_index: usize,
    setup: &DrmOutputSetup,
) -> bool {
    let mut add = {
        let los = ctx.inner.layoutputs.borrow();
        los[lo_index].add.clone()
    };
    let mut failed: Vec<*mut weston_sys::weston_head> = Vec::new();

    try_attach(output, &mut add, &mut failed);
    if !configure(ctx, output, setup) {
        return false;
    }
    if !try_enable(ctx, output, &mut add, &mut failed) {
        return false;
    }

    // C registers a head tracker for every head that survived; the
    // registry entry + destroy listener here is that bookkeeping.
    for h in add.iter().filter_map(|p| NonNull::new(*p)) {
        let name = crate::compositor::head_name(h);
        crate::compositor::register_head(ctx, h, &name);
    }

    // Push failed heads to the next round (C `lo->add = failed`).
    ctx.inner.layoutputs.borrow_mut()[lo_index].add = failed;
    true
}

/// C `drm_process_layoutput` (main.c:2894):
///
/// ```text
/// For each existing output of this group:  try attach
/// While heads are left to enable:          create output, attach, enable
/// ```
fn process_one(ctx: &Ctx, lo_index: usize, policy: &OutputPolicy, current_mode: bool) -> bool {
    // Existing outputs first: a head that arrives later can often just
    // attach to the output its clones already brought up.
    let existing: Vec<Rc<LayoutputOutput>> = ctx.inner.layoutputs.borrow()[lo_index]
        .outputs
        .iter()
        .map(Rc::clone)
        .collect();
    for entry in existing {
        let Some(output) = entry.get() else {
            // C: clean up left-overs from destroyed heads.
            remove_output(ctx, lo_index, &entry);
            continue;
        };
        let mut add = ctx.inner.layoutputs.borrow()[lo_index].add.clone();
        let mut failed = Vec::new();
        try_attach(output, &mut add, &mut failed);
        ctx.inner.layoutputs.borrow_mut()[lo_index].add = failed;
        if ctx.inner.layoutputs.borrow()[lo_index].add.is_empty() {
            return true;
        }
    }

    let (lo_name, section_head) = {
        let los = ctx.inner.layoutputs.borrow();
        (
            los[lo_index].name.clone(),
            los[lo_index].section_head.clone(),
        )
    };
    let compositor = ctx.inner.compositor.get();
    // C only reuses the plain group name when no output already owns
    // it; otherwise the new output gets "<group>:<head>".
    let cname = CString::new(lo_name.clone()).unwrap_or_else(|_| c"output".into());
    // SAFETY: compositor live; pure lookup.
    let name_taken =
        !unsafe { weston_sys::weston_compositor_find_output_by_name(compositor, cname.as_ptr()) }
            .is_null();
    let mut reuse_name = !name_taken;

    while !ctx.inner.layoutputs.borrow()[lo_index].add.is_empty() {
        if !ctx.inner.layoutputs.borrow()[lo_index].outputs.is_empty() {
            log::log_line("Error: independent-CRTC clone mode is not implemented.");
            return false;
        }
        let first = ctx.inner.layoutputs.borrow()[lo_index].add[0];
        let Some(first) = NonNull::new(first) else {
            return false;
        };
        let name = if reuse_name {
            reuse_name = false;
            lo_name.clone()
        } else {
            format!("{}:{}", lo_name, crate::compositor::head_name(first))
        };
        let Some(entry) = create_output_with_head(ctx, lo_index, &name, first) else {
            return false;
        };
        let Some(output) = entry.get() else {
            return false;
        };

        // The setup is decided per *flush*, from the controlling
        // section — the same lookup C does with lo->section.
        let setup = match policy.decide_drm(&section_head, false, current_mode) {
            crate::output_policy::DrmHeadPlan::Enable { setup, .. } => setup,
            // Unreachable: a group only exists because a head reached
            // Enable.  Fall back to defaults rather than skip config.
            _ => policy.drm_defaults(current_mode),
        };
        if !try_attach_enable(ctx, output, lo_index, &setup) {
            destroy_output(ctx, lo_index, &entry);
            return false;
        }
    }
    true
}

/// C `drm_process_layoutputs` (main.c:2958) + the `require_outputs`
/// verdict.  Returns false when startup must fail.
pub(crate) fn process_all(
    ctx: &Ctx,
    policy: &OutputPolicy,
    current_mode: bool,
    require_outputs: RequireOutputs,
) -> bool {
    let mut failed = 0usize;
    let count = ctx.inner.layoutputs.borrow().len();
    for i in 0..count {
        if ctx.inner.layoutputs.borrow()[i].add.is_empty() {
            continue;
        }
        if !process_one(ctx, i, policy, current_mode) {
            ctx.inner.layoutputs.borrow_mut()[i].add.clear();
            failed += 1;
        }
    }
    match require_outputs {
        RequireOutputs::AllFound => failed == 0,
        // C: `failed == wl_list_length(&layoutput_list)`.  Note that an
        // EMPTY list satisfies this too (0 == 0) — which is how
        // `mode=off` on the only head becomes a startup failure rather
        // than a compositor with no outputs.
        RequireOutputs::Any => failed != count,
        RequireOutputs::None => true,
    }
}

/// C `enum require_outputs` (main.c:136).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RequireOutputs {
    /// C REQUIRE_OUTPUTS_ANY — the default.
    #[default]
    Any,
    AllFound,
    None,
}

impl RequireOutputs {
    /// C `require_outputs_modes[]` names, exactly.
    pub fn parse(s: &str) -> Option<RequireOutputs> {
        Some(match s {
            "any" => RequireOutputs::Any,
            "all" => RequireOutputs::AllFound,
            "none" => RequireOutputs::None,
            _ => return None,
        })
    }
}

/// C `wet_layoutput_create_output_with_head` (main.c:2647).
fn create_output_with_head(
    ctx: &Ctx,
    lo_index: usize,
    name: &str,
    head: NonNull<weston_sys::weston_head>,
) -> Option<Rc<LayoutputOutput>> {
    let compositor = ctx.inner.compositor.get();
    let cname = CString::new(name).unwrap_or_else(|_| c"output".into());
    // SAFETY: compositor + head live inside the flush.
    let output = unsafe {
        weston_sys::weston_compositor_create_output(compositor, head.as_ptr(), cname.as_ptr())
    };
    let output = NonNull::new(output)?;

    let slot: Rc<Cell<*mut weston_sys::weston_output>> = Rc::new(Cell::new(output.as_ptr()));
    let slot_for_cb = Rc::clone(&slot);
    let destroy = Listener::new(
        "layoutput_output_destroy",
        true,
        Box::new(move |_ctx, _data| {
            // C wet_output_handle_destroy nulls output->output; the
            // entry survives so process_one can reap it next flush.
            slot_for_cb.set(std::ptr::null_mut());
        }),
    );
    // SAFETY: output just created; the listener box outlives the
    // attachment (held by the entry below).
    unsafe {
        weston_sys::weston_output_add_destroy_listener(output.as_ptr(), destroy.raw_ptr());
    }
    destroy.mark_attached();

    let entry = Rc::new(LayoutputOutput {
        output: slot,
        _destroy: destroy,
    });

    ctx.inner.layoutputs.borrow_mut()[lo_index]
        .outputs
        .push(Rc::clone(&entry));
    Some(entry)
}

/// C `drm_head_disable`'s tail (main.c:2985): the caller has already
/// detached the head; if that was the output's last one, the output
/// goes with it.
pub(crate) fn head_detached(ctx: &Ctx, output: NonNull<weston_sys::weston_output>) {
    if count_heads(output) != 0 {
        return;
    }
    let found = {
        let los = ctx.inner.layoutputs.borrow();
        los.iter().enumerate().find_map(|(i, lo)| {
            lo.outputs
                .iter()
                .find(|e| e.get() == Some(output))
                .map(|e| (i, Rc::clone(e)))
        })
    };
    if let Some((lo_index, entry)) = found {
        destroy_output(ctx, lo_index, &entry);
    }
}

/// Drop an entry whose output already died (C's "clean up left-overs").
fn remove_output(ctx: &Ctx, lo_index: usize, entry: &Rc<LayoutputOutput>) {
    let mut los = ctx.inner.layoutputs.borrow_mut();
    los[lo_index].outputs.retain(|e| !Rc::ptr_eq(e, entry));
}

/// C `wet_output_destroy` (main.c:2691): the output may be destroyed
/// lazily by the backend, so C forces the destroy callback first and
/// then destroys.  Here the listener does the nulling either way.
fn destroy_output(ctx: &Ctx, lo_index: usize, entry: &Rc<LayoutputOutput>) {
    if let Some(output) = entry.get() {
        entry.output.set(std::ptr::null_mut());
        // SAFETY: output live and owned by us.
        ctx.with_depth(|| unsafe { weston_sys::weston_output_destroy(output.as_ptr()) });
    }
    remove_output(ctx, lo_index, entry);
}

/// C `drm_backend_output_configure` (main.c:2333), the part that talks
/// to the backend.  The policy decided *what*; this applies it.
fn configure(
    ctx: &Ctx,
    output: NonNull<weston_sys::weston_output>,
    setup: &DrmOutputSetup,
) -> bool {
    let compositor = ctx.inner.compositor.get();
    // weston_drm_output_get_api is a static inline (§3k): call the
    // plugin lookup directly with the installed header's API name.
    // SAFETY: compositor live; name/size come from the header
    // constants.
    let api = unsafe {
        weston_sys::weston_plugin_api_get(
            compositor,
            c"weston_drm_output_api_v1".as_ptr(),
            std::mem::size_of::<weston_sys::weston_drm_output_api>(),
        )
    }
    .cast::<weston_sys::weston_drm_output_api>();
    if api.is_null() {
        log::log_line("Cannot use weston_drm_output_api.");
        return false;
    }

    let modeline = match &setup.mode {
        DrmMode::Modeline(s) => CString::new(s.as_str()).ok(),
        _ => None,
    };
    let mode_c = match setup.mode {
        DrmMode::Current => {
            weston_sys::weston_drm_backend_output_mode::WESTON_DRM_BACKEND_OUTPUT_CURRENT
        }
        _ => weston_sys::weston_drm_backend_output_mode::WESTON_DRM_BACKEND_OUTPUT_PREFERRED,
    };

    // SAFETY: api non-null and fully populated by the DRM backend;
    // output live and not yet enabled; the strings outlive the calls
    // (the backend copies what it keeps, as in C where these are
    // free()d immediately after).
    unsafe {
        let set_mode = (*api).set_mode.expect("drm set_mode");
        if set_mode(
            output.as_ptr(),
            mode_c,
            modeline.as_ref().map_or(std::ptr::null(), |c| c.as_ptr()),
        ) < 0
        {
            let name = crate::compositor::output_name(output);
            log::log_line(&format!(
                "Cannot configure output \"{name}\" using weston_drm_output_api."
            ));
            return false;
        }
        ((*api).set_max_bpc.expect("drm set_max_bpc"))(output.as_ptr(), setup.max_bpc);

        weston_sys::weston_output_set_scale(output.as_ptr(), setup.scale);
        // C: with a single head the default transform is the HEAD's own
        // (weston_head_get_transform), not NORMAL — a panel mounted
        // rotated reports it and the output must follow.
        let transform = match setup.transform {
            Some(t) => t.to_c(),
            None => head_transform(output),
        };
        weston_sys::weston_output_set_transform(output.as_ptr(), transform);

        if let Some(fmt) = &setup.gbm_format {
            let c = CString::new(fmt.as_str()).unwrap_or_default();
            ((*api).set_gbm_format.expect("drm set_gbm_format"))(output.as_ptr(), c.as_ptr());
        } else {
            ((*api).set_gbm_format.expect("drm set_gbm_format"))(output.as_ptr(), std::ptr::null());
        }

        if let Some(ct) = &setup.content_type {
            let c = CString::new(ct.as_str()).unwrap_or_default();
            if ((*api).set_content_type.expect("drm set_content_type"))(output.as_ptr(), c.as_ptr())
                < 0
            {
                return false;
            }
        } else if ((*api).set_content_type.expect("drm set_content_type"))(
            output.as_ptr(),
            std::ptr::null(),
        ) < 0
        {
            return false;
        }

        // C always calls set_seat, with "" when the key is absent.
        let seat = CString::new(setup.seat.as_str()).unwrap_or_default();
        ((*api).set_seat.expect("drm set_seat"))(output.as_ptr(), seat.as_ptr());
    }
    true
}

/// The transform of the output's first head, when the output has
/// exactly one (C `count_remaining_heads(output, NULL) == 1` guard at
/// main.c:2384; with clones there is no single head to inherit from and
/// C leaves the default).
fn head_transform(
    output: NonNull<weston_sys::weston_output>,
) -> weston_sys::wl_output_transform::Type {
    // SAFETY: output live; both calls are pure queries.
    unsafe {
        let first = weston_sys::weston_output_get_first_head(output.as_ptr());
        if first.is_null() || count_heads(output) != 1 {
            return weston_sys::wl_output_transform::WL_OUTPUT_TRANSFORM_NORMAL;
        }
        weston_sys::weston_head_get_transform(first)
    }
}

/// C `count_remaining_heads(output, NULL)` (main.c:1894).
fn count_heads(output: NonNull<weston_sys::weston_output>) -> usize {
    let mut n = 0;
    let mut head: *mut weston_sys::weston_head = std::ptr::null_mut();
    loop {
        // SAFETY: output live; the iterator tolerates a null cursor.
        head = unsafe { weston_sys::weston_output_iterate_heads(output.as_ptr(), head) };
        if head.is_null() {
            return n;
        }
        n += 1;
    }
}
