//! `ShellHost`: the trait boundary the shell is written against
//! (plan §3e, D20/A5).
//!
//! Queries return `Option<plain data>`; commands take ids.  No wrapper
//! type, no pointer, no `weston_sys` name crosses this trait — the
//! shell crate stays `#![forbid(unsafe_code)]` and unit-tests against a
//! mock implementation.
//!
//! R0 defines the core query surface the primitives can already serve;
//! the R1 shell port grows it (activate, move_to_layer, start grabs,
//! set_size, …) — each addition lands with its callback-inventory row.

use crate::ctx::Ctx;
use crate::ids::OutputId;

/// Plain geometry (POD re-declared at the fence — §3h; no
/// `weston_geometry` leaks through).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputInfo {
    /// Owned at the fence (§3h): copied, lossy-UTF-8, never borrowed.
    pub name: String,
    pub geometry: Rect,
}

pub trait ShellHost {
    /// Snapshot of live outputs (§3l: snapshots, never live C iterators).
    fn outputs(&self) -> Vec<OutputId>;
    /// `None` when the id is stale — the caller skips, never panics
    /// (D13 `Option`-everywhere policy).
    fn output_info(&self, id: OutputId) -> Option<OutputInfo>;
}

impl ShellHost for Ctx {
    fn outputs(&self) -> Vec<OutputId> {
        self.inner
            .outputs
            .borrow()
            .live_ids()
            .into_iter()
            .map(OutputId)
            .collect()
    }

    fn output_info(&self, id: OutputId) -> Option<OutputInfo> {
        let ptr = self.inner.outputs.borrow().resolve(id.0)?;
        // SAFETY: resolve() returned it ⇒ the registry's destroy
        // listener has not fired ⇒ the weston_output is live; we read
        // POD fields and copy the name before returning (§3h).
        let (name, geometry) = unsafe {
            let o = ptr.as_ref();
            let name = if o.name.is_null() {
                String::new()
            } else {
                std::ffi::CStr::from_ptr(o.name)
                    .to_string_lossy()
                    .into_owned()
            };
            (
                name,
                Rect {
                    x: o.pos.c.x as i32,
                    y: o.pos.c.y as i32,
                    width: o.width,
                    height: o.height,
                },
            )
        };
        Some(OutputInfo { name, geometry })
    }
}
