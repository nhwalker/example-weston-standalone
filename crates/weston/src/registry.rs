//! The per-kind slot table behind generational ids (plan §3b).
//!
//! One `SlotTable<T>` per object kind.  A slot holds the raw pointer and
//! the generation ids were minted with; invalidation clears the pointer
//! and bumps the generation, permanently staling every id minted for the
//! old occupant.  Live generations start at 1 and never revisit 0, so a
//! generation-0 id (the `test-ids` forge, a zeroed id) can never alias a
//! live entry.  The reverse direction (pointer → id) is a linear scan —
//! cold paths only (§3b); per-object user-data slots are a private
//! optimization added where profiling demands it.

use core::ptr::NonNull;

use crate::ids::RawId;

pub(crate) struct SlotTable<T> {
    slots: Vec<Slot<T>>,
    free: Vec<u32>,
}

struct Slot<T> {
    ptr: Option<NonNull<T>>,
    generation: u32,
}

impl<T> Default for SlotTable<T> {
    fn default() -> Self {
        SlotTable {
            slots: Vec::new(),
            free: Vec::new(),
        }
    }
}

impl<T> SlotTable<T> {
    /// Register a live C object; returns the id all cross-references use.
    ///
    /// Callers in `ctx.rs` pair every `insert` with the attachment of the
    /// destroy listener that will `invalidate` this slot — in the same
    /// function, so the table never holds an object without its
    /// invalidator (§3b).
    pub(crate) fn insert(&mut self, ptr: NonNull<T>) -> RawId {
        if let Some(slot) = self.free.pop() {
            let s = &mut self.slots[slot as usize];
            debug_assert!(s.ptr.is_none());
            s.ptr = Some(ptr);
            RawId {
                slot,
                generation: s.generation,
            }
        } else {
            let slot = u32::try_from(self.slots.len()).expect("slot table overflow");
            // Generation 0 is never minted (module doc): fresh slots
            // start live at 1.
            self.slots.push(Slot {
                ptr: Some(ptr),
                generation: 1,
            });
            RawId {
                slot,
                generation: 1,
            }
        }
    }

    /// Resolve an id to the live pointer, or `None` if the object died
    /// (or the slot was recycled for a new object).
    pub(crate) fn resolve(&self, id: RawId) -> Option<NonNull<T>> {
        let s = self.slots.get(id.slot as usize)?;
        if s.generation != id.generation {
            return None;
        }
        s.ptr
    }

    /// Invalidate the slot holding `ptr` (called synchronously from the
    /// object's destroy trampoline — §3e "sync invalidation half").
    /// Returns the staled id so the caller can build the deferred policy
    /// event.  A miss is a no-op: destroy signals can fire for objects
    /// we never registered.
    pub(crate) fn invalidate_ptr(&mut self, ptr: NonNull<T>) -> Option<RawId> {
        let idx = self.slots.iter().position(|s| s.ptr == Some(ptr))?;
        let s = &mut self.slots[idx];
        let staled = RawId {
            slot: idx as u32,
            generation: s.generation,
        };
        s.ptr = None;
        s.generation = s.generation.wrapping_add(1);
        if s.generation == 0 {
            // Keep the "generation 0 is never live" invariant even
            // across a (theoretical) u32 wrap.
            s.generation = 1;
        }
        self.free.push(staled.slot);
        Some(staled)
    }

    /// Pointer → id for a live object (cold paths: hotplug, trampolines).
    pub(crate) fn id_of(&self, ptr: NonNull<T>) -> Option<RawId> {
        self.slots.iter().enumerate().find_map(|(i, s)| {
            (s.ptr == Some(ptr)).then_some(RawId {
                slot: i as u32,
                generation: s.generation,
            })
        })
    }

    /// Snapshot of all live ids (§3l: C-list traversals surface as
    /// snapshots; this is the registry-side equivalent).
    pub(crate) fn live_ids(&self) -> Vec<RawId> {
        self.slots
            .iter()
            .enumerate()
            .filter(|(_, s)| s.ptr.is_some())
            .map(|(i, s)| RawId {
                slot: i as u32,
                generation: s.generation,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(v: usize) -> NonNull<u8> {
        NonNull::new(v as *mut u8).unwrap()
    }

    #[test]
    fn stale_id_resolves_none_after_invalidation() {
        let mut t = SlotTable::<u8>::default();
        let id = t.insert(p(0x1000));
        assert!(t.resolve(id).is_some());
        t.invalidate_ptr(p(0x1000)).unwrap();
        assert!(t.resolve(id).is_none());
    }

    #[test]
    fn recycled_slot_does_not_resurrect_old_id() {
        let mut t = SlotTable::<u8>::default();
        let old = t.insert(p(0x1000));
        t.invalidate_ptr(p(0x1000)).unwrap();
        // Same address recycled for a brand-new object — the C hazard the
        // generational design exists for (§3b).
        let new = t.insert(p(0x1000));
        assert_ne!(old, new);
        assert!(t.resolve(old).is_none());
        assert_eq!(t.resolve(new), Some(p(0x1000)));
    }

    #[test]
    fn invalidate_unknown_ptr_is_noop() {
        let mut t = SlotTable::<u8>::default();
        assert!(t.invalidate_ptr(p(0x2000)).is_none());
    }

    #[test]
    fn generation_zero_is_never_live() {
        // The test-ids forge (and any zeroed id) uses generation 0; it
        // must never alias a real entry, fresh or recycled.
        let mut t = SlotTable::<u8>::default();
        let id = t.insert(p(0x1000));
        assert_ne!(id.generation, 0);
        let forged = RawId {
            slot: id.slot,
            generation: 0,
        };
        assert!(t.resolve(forged).is_none());
        t.invalidate_ptr(p(0x1000)).unwrap();
        let recycled = t.insert(p(0x3000));
        assert_ne!(recycled.generation, 0);
        assert!(t.resolve(forged).is_none());
    }
}
