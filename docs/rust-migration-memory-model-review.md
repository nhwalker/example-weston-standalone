# Review: memory-model fencing in the Rust migration plan

Status: **proposed changes to `docs/rust-migration-plan.md`. This
document does not modify the plan** — the plan is unchanged and should
be read alongside this file. Every item here is a proposed edit, with
its target plan section named; §17 is the consolidated edit map and
decision list.

Reviews `docs/rust-migration-plan.md` (§2, §3, §6, §9) against the
actual C sources in this repo and the installed libweston 14 API, with
one question in focus: *does the plan, as written, guarantee that all
complex memory models stay inside `weston-sys`/`weston`, and that the
four safe crates get the most straightforward model available?*

**Requirement agreed at review (2026-07-24), and the yardstick for
everything below:**

> **All `unsafe` *and all unsoundness* live in one crate.** `weston`'s
> public API must be sound for arbitrary safe callers — including
> callers that misuse it, store handles indefinitely, or hold handles
> to objects that have since died. It must not be possible to cause
> undefined behavior from a `#![forbid(unsafe_code)]` crate.

This is stronger than the plan's goal 1, which says only that all
`unsafe` lives in one crate. The difference matters: a fence around the
`unsafe` *keyword* is not a fence around unsoundness. If `weston`
accepts a handle that may be dangling and dereferences it, every
memory-safety bug in the system is still authorable from a crate that
claims to be safe, and `forbid(unsafe_code)` is decoration. Plan edit:
sharpen goal 1's wording to the paragraph above.

Verdict: the plan names the fence correctly and picks the right five
FFI primitives, but it under-specifies the fence in four ways that
decide the answer:

1. The handle model it proposes (`Copy` newtypes around `NonNull`) is
   **unsound as a safe API** — a stale handle used from a
   `#![forbid(unsafe_code)]` crate is UB (§1).
2. It never states the **ownership taxonomy** — which C objects we own,
   which own themselves, which we allocate for C to borrow — so there
   is no rule saying which wrapper type a given API gets (§3).
3. It never specifies the **call-in direction**: how safe code reaches
   libweston, and who holds the borrow while it does. §3(d)'s answer
   ("`RefCell`s inside a single-threaded state struct") puts borrow
   discipline *in the safe crates*, which is the opposite of the stated
   goal (§7, §8).
4. Several load-bearing C idioms in *our* sources do not survive the
   fence at all, and the plan does not say what replaces them:
   listener-identity lookup (§5), grab structs freed from inside their
   own callbacks (§6), our own `wl_signal` (§16), `va_list` handlers
   (§11).

Everything below is evidence-backed against file:line so the plan edits
can be made without re-deriving.

**All open choices were decided by the owner on 2026-07-25** — D13–D20,
collected in §17 with the rejected alternatives kept for the record. Two
picks came out stricter than this review recommended, and are flagged
where they land: no `unwrap`/`expect` anywhere in the safe crates
(§1.6), and all four fence-enforcement checks adopted rather than a
subset (§13).

## 1. The core soundness gap: `Copy` pointer handles

### 1.1 Why the current design breaks the requirement

§3(b) says: *"Handles in the safe layer are lightweight `Copy` newtypes
around `NonNull<T>`"*. Combined with the §2 rule that the safe crates
are `#![forbid(unsafe_code)]`, that is a contradiction: if
`Output(NonNull<weston_output>)` is `Copy` and `Output::width(&self)`
is a safe method that dereferences, then

```rust
// 100 % safe code, forbid(unsafe_code) crate — and UB
let out = shell.outputs[0];      // handle copied into shell state
// ... output unplugged, weston_output freed by libweston ...
let w = out.width();             // use-after-free
```

No `unsafe` block appears on the failing line, and no lint fires. The
`unsafe` that dereferences lives in `weston` and was written correctly
— it was handed a pointer that used to be valid.

Rust's usual protection does not apply here, and it is worth stating
plainly in the plan so the design is not mistaken for paranoia: the
borrow checker prevents use-after-free only for memory **Rust owns**.
These objects are owned and freed by C on its own schedule
(monitor unplug, client crash, deferred DRM teardown), which the
compiler cannot see. So the liveness check has to be built once, by
hand, inside the wrapper — it is not inherited from the language.

The C code carries the same hazard, and pays for it by hand at every
site:

- `destroy_shell_grab_shsurf()` nulls a back-pointer on death —
  `desktop-shell/shell.c:178`.
- `desktop_surface_removed()` nulls `shseat->focused_surface` on every
  seat, with a comment explaining the stale-pointer hazard —
  `desktop-shell/shell.c:1263-1291`.
- `shsurf->desktop_surface = NULL` leaves a *half-dead* shell surface
  that is still used afterwards — `desktop-shell/shell.c:1290`.

### 1.2 Two shapes satisfy the requirement; the choice is ergonomics

The requirement does not mandate ids. It mandates: *no safe crate may
hold something that can be dereferenced without a liveness check.* Two
shapes do that, and both are equally sound:

**(a) Generational ids + slot table** *(recommended)*

- `weston` keeps a per-kind slot table: `Vec<Slot>` where
  `Slot = { ptr: Option<NonNull<T>>, generation: u32 }`.
- Safe crates hold `OutputId`, `SurfaceId`, `ViewId`, `SeatId`,
  `HeadId` — `Copy + Eq + Ord + Hash + Debug`, carrying
  `(slot, generation)` and **no pointer**.
- Resolution is an array index plus an integer compare — **not a hash
  lookup**. (Correcting a loose earlier phrasing: hashing is needed
  only for the reverse direction, pointer → id, which is a cold path
  only — see 1.5.)
- The generation is what closes the recycled-address hole: slot 12 is
  freed and reused by a new object, its generation goes 7 → 8, and
  every id minted for the old occupant is permanently stale. A `Copy`
  pointer handle cannot close this hole at all — two different objects
  really can share an address, so comparing two pointer handles can
  report a dead object and a live one as *equal*.
- Registration and destroy-listener attachment must happen in **one
  function**, so an object is never in the table without the listener
  that removes it. Otherwise a stale entry plus a recycled pointer
  arriving in a later callback resolves to the wrong object.

**(b) Invalidatable wrapper object** — `Rc<Cell<Option<NonNull<T>>>>`;
the destroy listener sets the cell to `None` once, and every holder
sees it. Resolution is a deref plus a null test.

| | (a) ids | (b) `Rc` cell |
|---|---|---|
| Resolve cost | ~1–2 ns (index + compare) | ~1–2 ns (deref + test) |
| Per-object allocation | none | one cell |
| Copy semantics | `Copy`, free to store anywhere | `Clone`, refcount traffic per copy |
| As a map key | trivial and correct | awkward (inner pointer → ABA hole; cell address → obscure) |
| Leak classes | none | `Rc` cycles possible |
| Reverse lookup (C → us) | slot id fits in a `void*` user-data slot | cell pointer in the user-data slot |

**Decided (D13): (a), generational ids** — chosen for shell ergonomics,
not for speed; the two shapes cost the same. Recorded with it: (b) is
**equally sound** and is a *wrapper-internal* change, so if shell code
reads badly, switching later is an implementation question and not a
safety one.

Also considered and rejected: `Rc`/`Weak` with `upgrade()` at every
call site (same as (b) plus plumbing); branded/`GhostCell` lifetimes
(sound, zero-cost, but viral lifetime parameters through all shell
code); shadow state that mirrors libweston in our own arenas (simplest
of all, but duplicated state and a synchronisation problem). Record the
rejections so they are not revisited.

### 1.3 Callback subjects are live by construction — do not check them

A refinement that removes most of the `Option` noise, and the answer to
"why would we ever be handed a stale id?": **the object a callback is
*about* is always alive.** libweston is calling us because something
happened to it; it has not been freed. So the fence has two handle
kinds, not one:

| What | Given to safe code as | Check needed |
|---|---|---|
| The callback's subject | a **live borrowed wrapper** (`Surface<'_>`, `Output<'_>`) valid for the call | **none** |
| Anything the shell **stored earlier** | an id | resolved on use |

A handler that only touches its own subject therefore has zero
liveness checks. Only cross-references — the focused surface, a parent,
the output a window was on, the grabbed surface — go through
resolution. Plan edit: state both kinds explicitly in §3(b), because
the current text implies one uniform handle type.

### 1.4 Where staleness genuinely comes from

Six structural sources, five of them already live in the C code. This
list is the justification for checked cross-references; without it the
checks look like defensive programming:

1. **Cleanup is itself a callback, and other state still points at the
   dying object.** `desktop_surface_removed()` walks every seat nulling
   `focused_surface` (`shell.c:1263-1291`) — the comment says otherwise
   `activate()` touches a just-removed surface. Same shape in the
   children-list fixup (`shell.c:213-217`) and in
   `focus_state_surface_destroy()` (`shell.c:362-394`), which runs
   *inside* a destroy handler and goes looking for another window to
   focus.
2. **Our objects deliberately outlive the C objects.**
   `shsurf->desktop_surface = NULL` (`shell.c:1290`) — the shell
   surface is used for the rest of teardown after the C object is gone.
3. **libweston destroys things in the middle of our outbound calls.**
   `shell_grab_start()` calls `weston_seat_break_desktop_grabs()`
   (`shell.c:238`), which synchronously cancels and frees other grabs,
   running our handlers before returning — so ids can go stale between
   two statements of one function.
4. **libweston's own destruction is sometimes deferred.**
   `wet_output_destroy()` (`main.c:2688-2694`): *"output->output
   destruction may be deferred in some cases (see drm_output_destroy()),
   so we need to forcibly trigger the destruction callback now, or
   otherwise would later access data that we are about to free."*
5. **Some objects never announce death.** `weston_keyboard` and
   `weston_touch` have no destroy signal (§2), so
   cleanup-on-notification is impossible for them by construction.
6. **Deferred dispatch (§8) creates a new window by design.** A queued
   click drained after the C frame returns can target an object that
   died in between. This is the reason deferral and checked ids belong
   together.

The registry does not *replace* the cleanup those C sites do — it does
the same cleanup once, centrally. What changes is the failure mode when
a cleanup site is missed:

| | A missed cleanup site |
|---|---|
| C today, and `Copy` pointer handles | use-after-free: silent, or a crash later in unrelated code |
| Checked ids | resolution returns "gone": skipped work, one log line |

Roughly a dozen scattered hand-cleanup sites today become one. The
check is what stops "did we cover every site?" from being a correctness
cliff in teardown paths — the least-tested code in the shell.

### 1.5 Cost, with numbers

Raised at review: a compositor is performance-critical and some
callbacks run at high rates. Order-of-magnitude estimates for *this*
compositor (not measurements):

| Callback | Realistic peak rate |
|---|---|
| Pointer motion (1 kHz mouse) | 125–1000/s |
| Surface commits (20 clients @ 60 Hz) | ~1200/s |
| Output repaint | 60–144/s per output |
| Keyboard, focus, hotplug, seat caps | <100/s |

A few thousand callbacks per second, against resolution costs of ~1–2 ns
(slot table or `Rc` cell) and ~5–25 ns (hash lookup, `FxHash`/`SipHash`).
At 5,000 callbacks/s the *expensive* option costs ~125 µs per second —
about 0.01 % of one core, against 1–5 ms of real work per repaint.

Three design points keep it there:

- **Resolve once per callback, then borrow.** The subject arrives
  already resolved (1.3); a cross-reference is resolved once and used as
  a borrowed wrapper for the rest of the handler — one check per
  callback, not one per field access.
- **No lookup at all on the two hottest entry paths.** Listener
  callbacks arrive via the `container_of` trampoline, which hands us our
  own struct directly (the listener *is* the back-pointer). Desktop
  surfaces — the once-per-client-per-frame `committed` path — use
  libweston's one real user-data slot, and an id fits in that
  pointer-sized word with no allocation.
- **The reverse lookup (pointer → id) is cold-path only**: output
  created/destroyed, head hotplug, seat added.

Plan edit: put these three points in §3(b) explicitly, so the cost
question is settled once rather than re-litigated as a "hot path"
optimisation during implementation. If a genuinely hot loop appears, the
escape hatch is a longer-lived `&Ctx`-scoped borrow (§7) or iterating
the wrapper's own already-resolved records — **never** a raw pointer
stored in safe state.

**Where the per-event cost actually is** (§10 collects these as rules):
string marshalling per commit, a `Vec` allocation per list snapshot, an
allocation per deferred event, and eager log formatting. Each is
100–1000× the resolution cost.

### 1.6 Stale cross-reference policy

**Decided (D13): `Option` everywhere, no exceptions.** Every resolution
is checked and handled; **no `unwrap` or `expect` on a value obtained
from `weston` appears anywhere in the safe crates**, enforced by
`clippy::unwrap_used` and `clippy::expect_used` denied at crate level
with no `allow` escapes. This is stricter than this review proposed
(which would have permitted `expect` where an invariant is provable) —
recorded deliberately: the argument that carried it is that a panic in a
compositor kills the user's session, and "provable" invariants in
teardown paths are exactly the ones that stop being true after a
refactor.

Consequences to write into the plan:

- The normal shape is `let Some(s) = cx.surface(id) else { return };` —
  a missed cleanup site degrades to skipped work plus one log line.
- Functions that must yield a value need a **documented fallback**
  rather than a panic. The one place the C code asserts on exactly this
  is `get_output_work_area()`, which zero-initialises the rectangle
  (`shell.c:257-260`) and then `assert(sh_output)` (`shell.c:266`); the
  Rust version logs and returns the zeroed rectangle. That is a
  deliberate divergence from the C behavior under `NDEBUG` (which
  continues and fills the area from the still-valid `weston_output`), so
  it must be recorded in the porting PR under the plan's D6.
- The rule bans *choosing* panic as the stale-reference policy; it does
  not ban panics generally. The `catch_unwind` trampoline barrier (D16)
  still exists for genuine bugs, and `debug_assert!` remains available
  for invariants that are not resolution results.

### 1.7 Design-level enforcement options (summary)

For the record, the full menu considered against the requirement, since
only the first lever can give the safe crates a simple memory model
(the second only prevents regressions):

| Option | Enforces soundness how | Safe-side model |
|---|---|---|
| Generational ids + slot table *(rec.)* | dead id resolves to `None`; no deref | plain structs + maps; no lifetimes, `Rc`, or `RefCell` |
| Invalidatable `Rc` cell | cell nulled on death | sound; `Clone` + refcount + cycle risk |
| Scoped borrows only | handle cannot outlive the callback | simplest, but nothing can be stored across dispatches — correct for `weston_keyboard`/`weston_touch` (§2), unusable alone |
| Shadow state (mirror C state in our arenas) | safe code never names a C object | simple, but duplicated state and sync risk |
| Branded lifetimes / `GhostCell` | compile-time validity proof | sound, zero-cost, viral lifetimes — rejected on readability |
| `Copy` raw handles + review discipline *(plan today)* | nothing | rejected: unsound |

**Decided:** ids for stored state, scoped borrows for objects that
cannot be invalidated (§2), live borrows for callback subjects (§1.3).
Mechanical enforcement — dependency-graph rule, public-API snapshot,
opaque id types, clippy unsafe lints, zero-exception
`forbid(unsafe_code)`, ASAN/destroy-storm testing — all adopted; see
§13.

### 1.8 Plan edits

- **Goal 1** — restate as the requirement quoted at the top of this
  document (unsoundness, not just `unsafe`, is confined to `weston`).
- **§3(b)** — replace the `Copy`/`NonNull` sentence with: the two
  handle kinds (1.3); generational ids in a slot table with paired
  registration/destroy-listener attachment (1.2); the three cost points
  (1.5); and the rule that no `weston-sys` type, no `NonNull`, and no
  raw pointer appears in `weston`'s public API.
- **§9** — add risk **R-H**: *"`Copy` pointer handles would let the
  `forbid(unsafe_code)` crates author use-after-free; handles are
  generational ids resolved through the wrapper's registry, and callback
  subjects are passed as borrows that cannot escape the call."*
- **§10** — record **D13** (handle model) including the rejected
  alternatives, the (b)-is-equally-sound note, and the 1.6 policy.

## 2. Object-death inventory (the fact base the handle model needs)

Not every libweston object announces its death, and the plan's single
sentence about destroy signals hides three different regimes. Verified
against `include/libweston/libweston.h` and `desktop.h` in the
reference tree:

| Object | Death announced by | Handle kind |
|---|---|---|
| `weston_compositor` | `destroy_signal` (`add_destroy_listener_once`, libweston.h:2443) | singleton, owned by `weston` |
| `weston_output` | `destroy_signal` + `weston_output_add_destroy_listener` (libweston.h:2479) | invalidatable id |
| `weston_head` | `destroy_signal` + `weston_head_add_destroy_listener` (libweston.h:2649) | invalidatable id |
| `weston_seat` | `destroy_signal` | invalidatable id |
| `weston_surface` | `destroy_signal` | invalidatable id |
| `weston_view` | `destroy_signal` | invalidatable id (we own most of ours — §3) |
| `weston_desktop_surface` | **no signal** — `surface_removed` API callback | id invalidated in the `surface_removed` trampoline |
| `weston_pointer` | `destroy_signal` | see below |
| `weston_keyboard` | **no destroy signal** | **dispatch-scoped borrow only** |
| `weston_touch` | **no destroy signal** | **dispatch-scoped borrow only** |
| `weston_layer` | no signal — *we* own it | owned/pinned (§3) |

Two normative rules fall out, neither of which is in the plan:

- **Input sub-objects (`weston_pointer`/`keyboard`/`touch`) are never
  stored.** They come and go with seat capability changes and cannot be
  invalidated reliably. The safe layer must re-fetch them from a
  `SeatId` at each use (which is exactly what the C code does —
  `weston_seat_get_pointer()` at `desktop-shell/shell.c:1135`), and the
  wrapper should express them as a borrow that cannot escape the
  callback (`fn with_pointer<R>(&self, seat: SeatId, f: impl FnOnce(Pointer<'_>) -> R) -> Option<R>`).
- **`weston_desktop_surface`'s "half-dead" window is explicit.** After
  `surface_removed`, our per-surface state still exists while the
  desktop surface is gone (`shell.c:1288-1291`). Model it as
  `Option<DesktopSurfaceId>` inside the shell's own struct, and state
  that the shell may observe its own state after the C object is dead —
  the id is `None`, not stale.

## 3. Ownership taxonomy (missing entirely)

§3(b) discusses only *C-owned* objects. Our sources use five distinct
ownership relationships, and each needs a different wrapper shape.
Without this table, the port will pick per-call-site and the fence will
leak.

| Kind | Examples (evidence) | Wrapper type | Safe-side view |
|---|---|---|---|
| 1. C owns, announces death | outputs, heads, seats, surfaces | registry id | `Option<…>` accessors |
| 2. C allocates on request, **we** destroy | `weston_desktop_surface_create_view` → `unlink_view` → `weston_view_destroy` ordering (`shell.c:218-220`); `weston_shell_utils_curtain_create/destroy` (`shell.c:1894, 1960`); log scopes, recorder | `Owned<T>` RAII, `Drop` = destroy in the documented order | an id that the shell may explicitly drop |
| 3. **We** allocate, C borrows by address | `wl_listener` (32 fields across our sources); grab structs (`shell.c:117-140`); `weston_desktop_api` vtable; `weston_*_backend_config` | `Pin<Box<…>>` inside the wrapper, never exposed | nothing — invisible |
| 4. C struct **embedded by value** in our allocation | `struct weston_layer` inside `struct workspace` (`shell.h:34`) and `desktop_shell::background_layer` (`shell.h:60`) | pinned, address-stable field; must not move after `weston_layer_init` | `LayerId` |
| 5. C allocates per-call, valid for the callback only | event args (`const struct timespec *`, coords), string getters | borrow with a lifetime, or copied at the fence | owned values (§10) |

Kind 4 deserves a call-out in the plan: `weston_layer_init` registers
an intrusive list node *at the struct's address*. A Rust port that
holds `weston_layer` in a movable struct (a `Vec<Workspace>` that
reallocates, a returned-by-value builder) silently corrupts libweston's
layer list with no compile error and no immediate crash. Rule:
**every kind-3/kind-4 allocation lives behind `Pin<Box<…>>` created
inside `weston`; the safe crates cannot name the type.**

## 4. Only two user-data slots exist — §3(b) is written backwards

§3(b) presents the user-data slot as the normal mechanism and the
pointer-keyed map as the fallback ("where libweston gives no user-data
slot"). The installed API has exactly two slots:

- `weston_desktop_surface_{set,get}_user_data` (desktop.h:154, 187)
- `weston_compositor_get_user_data` (libweston.h:2474) — get only

Everything else (output, head, seat, surface, view, layer) has **no**
slot. So the map is the rule and the slot is the exception — and since
§1 already introduces a registry, the plan should say there is **one**
mechanism (the registry), with the desktop-surface slot used only as an
optimisation for the hottest lookup (`get_shell_surface()`,
`shell.c:1197-1206`), if at all. One mechanism is one place to audit.

Related R1 hazard: in the hybrid phase the compositor's user-data slot
belongs to the **C** frontend (`wet_get_config` reaches it via
`weston_compositor_get_user_data`, `main.c:363`). The Rust shell must
not read, write, or assume that slot; it gets its root state from its
own `wet_shell_init` allocation. Worth one sentence in §7/R1 — it is
the kind of thing that appears to work in testing and corrupts under a
different load order.

## 5. Listener-identity lookup does not survive the `Listener<Args>` design

Three sites use *the notify function pointer as a lookup key* and the
`wl_listener` itself as a user-data slot:

- `get_shell_seat()` → `wl_signal_get(&seat->destroy_signal, destroy_shell_seat)`
  then `container_of` — `desktop-shell/shell.c:1179-1193`
- `wet_head_tracker_from_head()` → `weston_head_get_destroy_listener(head, handle_head_destroy)`
  — `frontend/main.c:1940-1950`
- `wet_output_from_weston_output()` → `weston_output_get_destroy_listener(base, wet_output_handle_destroy)`
  — `frontend/main.c:2674-2684`

§3(a)'s design — one `extern "C"` trampoline shared by all listeners,
dispatching to a boxed closure — collapses all three keys to a single
address, so the idiom becomes ambiguous. A generic
`extern "C" fn trampoline::<F>` gives distinct addresses per closure
type *in practice*, but identical-code folding (linker ICF, and Rust's
own function-item deduplication) makes that unspecified; do not build
on it.

Normative text to add: **listener identity is never a lookup key. All
three lookups become registry lookups keyed by the C object pointer,
and `weston`'s public API never exposes a listener's identity or
address.** This is a straight simplification for the safe crates —
`get_shell_seat()` becomes `self.seats.get(&seat_id)`.

While specifying `Listener`, also cover **conditional attachment**,
which the plan omits: `shell_seat_caps_changed()` uses
`wl_list_empty(&listener.link)` as "am I attached?" state and
attaches/detaches as pointer capability comes and goes
(`shell.c:1129-1145`). So `Listener` needs explicit
`attach`/`detach`/`is_attached`, `Drop` must be correct in both states,
and `wl_list_init`-after-remove (the C idiom that makes remove
idempotent) becomes an internal invariant of the primitive, not a
pattern safe code repeats.

## 6. Grabs are the missing sixth primitive — and the sharpest one

§3 claims its five patterns "cover essentially every unsafe
interaction". Grabs are not among them, and they combine every hard
property at once:

- Our allocation embeds the C struct and is handed to libweston by
  address: `struct shell_grab { struct weston_pointer_grab grab; … }`
  (`shell.c:117-128`), plus `weston_move_grab`/`weston_touch_move_grab`
  wrapping it again (`shell.c:130-140`).
- C holds that address for the grab's duration
  (`weston_pointer_start_grab`, `shell.c:247`).
- **The allocation is freed from inside its own callback**, nine sites:
  `shell.c:660, 671, 798, 815, 881, 914, 1524` (pointer) and
  `shell.c:530, 564` (touch) — e.g. `move_grab_button()` calls
  `shell_grab_end()` then `free(grab)` while C is dispatching on it.
- Starting a grab can synchronously **cancel and free a different
  grab**: `weston_seat_break_desktop_grabs()` (`shell.c:238, 296`)
  re-enters our `*_grab_cancel` → `free`.
- The grab holds a weak back-reference nulled on surface death
  (`shell.c:177-186`).
- The C free relies on the embedded struct being at offset 0
  (`free(grab)` where `grab` is the `weston_pointer_grab *`).

What the plan needs to specify:

- A `Grab` primitive in `weston` owning `Pin<Box<GrabInner<S>>>`, with
  the vtable filled by static trampolines (same shape as §3(c)).
- A **deferred-drop protocol**: `end_grab()` moves the box into the
  wrapper's "pending drop" slot; the box is dropped after the
  trampoline frame returns (drain at end of dispatch). Never
  `Box::from_raw` inside the callback that C is dispatching on.
- Offset math in exactly one place; never offset-0 casting.
- The back-reference is `Option<SurfaceId>` (§1), so "grabbed surface
  died mid-grab" is a `None`, not a nulled pointer.
- Safe-side view: the shell stores
  `enum ActiveGrab { None, Move { surface: SurfaceId, delta: … }, Resize { … }, Busy { … } }`
  in its own plain state. It never sees the C allocation, never frees
  anything, and cannot express "free while dispatching".

This is the single richest test of the wrapper design, which is a good
argument for the plan's D4 (shell first) — worth saying so in §7/R1,
and worth requiring a grab stress case (start/cancel/destroy races) in
the R1 exit criteria.

## 7. The call-in direction is unspecified (and it is where the goal is won)

The plan specifies callbacks *in* (§3a, §3c) and never specifies how
safe code calls libweston *out*. That omission is why §3(d) lands on
`RefCell`s in the safe layer: with no context object, the safe state
must own everything and re-enter itself.

Specify a context token:

```rust
// in `weston`
pub struct Ctx { /* registry, pending-drop queue, PhantomData<*const ()> */ }

impl Ctx {
    pub fn output(&self, id: OutputId) -> Option<Output<'_>>;
    pub fn view_move_to_layer(&self, view: ViewId, layer: LayerId) -> Result<(), Dead>;
    // …
}
```

and the shell-side signature shape:

```rust
// in `westonite-shell` — forbid(unsafe_code)
impl Shell {
    fn on_event(&mut self, cx: &Ctx, ev: ShellEvent) { … }
}
```

Then state the invariant that makes the safe crates simple — the
sentence the plan is currently missing:

> **One borrow at the edge.** Each trampoline takes exactly one
> `RefCell<AppState>` borrow, calls one safe method with `&mut self`,
> and releases it before returning to C. Consequently the safe crates
> contain **no interior mutability at all**: no `RefCell`, no `Rc`, no
> `Cell` — just `&mut self` over plain structs, maps, and ids.

Compare §3(d) as written ("all mutable state lives in `RefCell`s
inside a single-threaded state struct; borrows are scoped to never span
a call back into libweston"): that is the same discipline, but enforced
by review inside application code, with `BorrowMutError` aborts as the
failure mode. The edge version moves the whole problem into `weston`
where the unsafe code already lives. It is strictly the plan's own
stated goal, applied to interior mutability as well as to `unsafe`.

The one-borrow rule is only achievable if outbound calls cannot
synchronously re-enter our callbacks while the borrow is live — which
is §8.

## 8. Reentrancy: choose deferral over discipline, and inventory the callbacks

§3(d) accepts reentrancy and converts its failures into
`BorrowMutError` panics, calling that "strictly an improvement". It is
an improvement over UB, but in a compositor it is still an abort: the
session dies. And it is avoidable for most callbacks.

**Decided (D14): two-tier dispatch**, with each callback classified in
the plan. The alternatives considered and rejected: keeping §3(d)'s
`RefCell`-discipline-by-review, and a middle option (synchronous
inbound, deferred outbound effects only) that would have avoided the
event queue but left a per-call-site judgement about which outbound
calls re-enter.

- **Deferrable tier** — trampoline pushes an `Event` onto a queue; the
  wrapper drains the queue with no C frame on the stack, calling the
  shell with `&mut self`; outbound effects are applied through `&Ctx`
  after the borrow is released. Candidates: key/button/axis/debug
  bindings (6 sites), focus and destroy notifications, session,
  transform and resize signals, seat/output create.
- **Sync tier** — must answer inside the C call, because C reads a
  return value or an out-parameter immediately. These are the ones the
  plan must name, because each is a hand-audited exception to the
  one-borrow rule:
  - `weston_desktop_api::get_position` — out-params `int32_t *x, *y`
    (desktop.h:125)
  - `weston_surface_set_label_func` — writes into `char *buf, size_t len`
    (`shell.c:1238, 1289`)
  - `weston_curtain_params::get_label` (`shell.c:1926-1952`,
    shell-utils.h:33)
  - log handlers `vlog`/`vlog_continue` returning a char count
    (`main.c:214, 241, 4511`) — see §11
  - xwayland `spawn_xserver`, which must fork/exec and return the new
    `struct wl_client *` synchronously (`frontend/xwayland.c:105`,
    xwayland-api.h:48-50)
  - head-changed / output-configure callbacks, which must configure the
    output before returning (`main.c` `wet_head_tracker_*`,
    `simple_output_configure` in `struct wet_backend`, `main.c:129-134`)
  - grab motion/button/cancel — must move views within the frame
    (§6)

Deliverable to add to R0: a **callback inventory table** — every
`wl_listener` (32 fields), every attach site (23), the 6 bindings, and
the 14 `weston_desktop_api` entries — each with its tier, its
reentrancy hazards, and, for sync-tier entries, the borrow it is
allowed to take. That table is the audit surface for the whole fence;
it is small enough to be exhaustive and it is exactly what a reviewer
needs to check the "no unsafe outside `weston`" claim.

Deferral changes *ordering*, which the plan must acknowledge: e.g.
activation on focus change, and destroy-before-use sequences, are
observable. The e2e suite is the right oracle (that is its job per §3
of the plan) — but note explicitly that any behavior difference caused
by deferral is a **parity bug**, not an accepted divergence under D6.

## 9. Iterator invalidation across the fence

The C code walks libweston's intrusive lists while mutating them and
uses `wl_list_for_each_safe` to survive it:
`desktop_shell_destroy_surface()` (`shell.c:213`), the per-seat
invalidation loop in `desktop_surface_removed()` (`shell.c:1272`), and
the layer view walk in `focus_state_surface_destroy()`
(`shell.c:375-384`) which then mutates focus state.

Goal 2 of the plan says "iterators over intrusive-list macros" — that
is the right direction but underspecified in the dangerous case. Rule
to add: **all C-list traversals exposed to the safe layer are
snapshots** (`Vec<Id>` / `SmallVec<[Id; N]>`), taken while no safe code
runs; a lazily-borrowed C-list iterator must never be live across a
call that can mutate the list. With ids this is naturally correct: a
snapshot entry that dies mid-iteration resolves to `None`, so the safe
loop skips it instead of dereferencing freed memory.

## 10. Boundary value rules (small, but this is where unsafety leaks as API shape)

Add a short subsection to §3 fixing these once, so 200 call sites do
not each decide:

- **No `weston-sys` type appears in `weston`'s public API.** POD
  geometry types (`weston_coord_global`, `weston_geometry`,
  `pixman_rectangle32_t`) are re-declared as plain Rust types and
  converted at the fence — never `transmute`d, or if transmuted, with
  layout assertions in `weston-sys` tests.
- **Nullable returns become `Option`.** `weston_seat_get_pointer/
  keyboard/touch`, `weston_head_get_output`, and our own
  `get_default_view()` (`shell.c:188-206`) all return NULL legitimately.
  The bindgen signature (`*mut T`) hides that; the wrapper must
  document nullability per function as part of its SAFETY notes.
- **Strings are owned at the fence.** `weston_head_get_name`,
  `weston_desktop_surface_get_title/get_app_id`: client-controlled,
  possibly NULL, not guaranteed UTF-8, and valid only until the object
  changes. Safe crates get `Option<String>` via `from_utf8_lossy` (say
  which, so log output is predictable), never a borrowed `&str` whose
  lifetime is really "until the next commit".
- **Int flags become `bitflags`/enums** at the fence (activate flags,
  resize edges, transforms, modifier masks) — already implied by goal 2
  and the `bitflags` dependency; state it as a fence rule so no
  `u32`-typed flag reaches the shell.

### 10a. Boundary cost rules (D19 — adopted)

Raised at review: the compositor is performance-critical. Handle
resolution is not where the cost is (§1.5) — **boundary marshalling
is**, at 100–1000× the per-call cost of a liveness check. Four rules,
worth stating in §3 so they are designed in rather than profiled out:

- **Cache strings; do not fetch per event.** `get_title`/`get_app_id`
  per commit means a `strlen` + copy + allocation on the hottest
  callback. Fetch on the change notification, cache in shell state.
- **Do not allocate a `Vec` per list snapshot** (§9). Use `SmallVec`
  with an inline capacity sized for the realistic count (outputs, seats,
  a window's children), or a scratch buffer reused across callbacks.
- **Do not allocate per deferred event** (§8). A reusable `VecDeque` of
  a fixed-size `Event` enum; keep `Event` small enough to stay inline
  (box the rare large variant instead of widening all of them).
- **Format log messages lazily**, behind a scope-enabled check —
  `weston_log_scope_is_enabled()` (weston-log.h:85) exists for exactly
  this, and the C code already guards its hot scopes with it
  (`main.c:221` for the flight recorder, `main.c:283` for the protocol
  scope). The safe logging macros must preserve that guard rather than
  formatting eagerly and discarding.

These are the only places in the port where a per-event allocation is
plausible; naming them keeps the "is this fast enough?" conversation
attached to the things that actually matter.

## 11. Variadic C APIs need the shim — the plan's shim scope is too narrow

§2 and §6 scope the `cc` shim to the ~28 `static inline` helpers.
But the log path is variadic:

- `custom_handler(const char *fmt, va_list arg)` — `main.c:172`
- `vlog(const char *fmt, va_list ap)` / `vlog_continue(...)` —
  `main.c:214, 241`, installed by `weston_log_set_handler(vlog, vlog_continue)`
  — `main.c:4511`
- forwarding to `weston_log_scope_vprintf(scope, fmt, arg)`

Stable Rust cannot define a function taking `va_list` (`c_variadic` is
unstable), so these handlers **must** be C shim functions that either
forward straight to `weston_log_scope_vprintf` or `vsnprintf` into a
buffer and call back into Rust with a `&str`. Extend the shim's stated
scope in §2/§6 to "static inlines **plus variadic/`va_list`
interfaces", and note the pkg-config/bindings tripwire covers it.

One more line worth adding: outbound `weston_log("%s", …)` — the safe
logging macros must format in Rust and pass the result as a `%s`
argument, never as the format string. Passing user/client-controlled
text (window titles, config values) as a format string is a live bug
class in C that a naive wrapper reintroduces.

## 12. Globals, threads, and why the id design is sound

`main.c` keeps process-global mutable state: `weston_logfile`,
`log_scope`, `protocol_scope`, `cached_tm_mday` (`main.c:159-162`).
Rules to state:

- No `static mut` anywhere. These live in `weston` (a `Log` object, or
  `thread_local!` + `RefCell`), and the safe crates receive handles.
- Wrapper types are `!Send + !Sync` (the plan says this) — but add the
  reason it composes with §1: **ids are inert data and may be `Send`,
  yet resolving one requires `&Ctx`, which is `!Send`.** So no thread
  can touch C state even if it somehow obtains an id. That is the
  argument that makes the whole handle design defensible; it belongs in
  `weston`'s crate-level invariant docs.
- Review rule: safe crates spawn no threads and use no
  `std::thread`/async runtime (already implied by "no tokio", make it
  explicit as a memory-model rule, not just a dependency rule).
- A `debug_assert!(on_main_thread())` at every `Ctx` entry point, kept
  enabled in CI builds.

## 13. Fence enforcement: make it structural, and say what validates the unsafe

§2 relies on `#![forbid(unsafe_code)]` plus a "CI grep". Both are weak
where it matters. **Decided (D17): all four checks below are adopted**
(the review offered them as a menu; the owner took the whole set), plus
the `clippy::unwrap_used`/`expect_used` denial that D13 implies (§1.6):

- **Dependency-graph rule**: the four safe crates must not depend on
  `weston-sys` at all (only on `weston`). Checked with `cargo metadata`
  in CI. This is the rule that actually prevents "safe" code from
  holding raw pointers, and it is unforgeable — unlike a grep.
- **Public-API rule**: `weston`'s public API contains no raw pointers,
  no `NonNull`, and no `weston_sys::*` types. Enforce with a
  `cargo public-api` snapshot test (also gives a reviewable diff every
  time the fence moves).
- **Opaque ids**: handle types have private fields, no public
  constructor, and no `From`/`Into` conversions to or from pointers or
  integers. Even with `weston` in scope, safe code then cannot fabricate
  a handle or unwrap one back into something dereferenceable — the §1
  soundness property cannot be bypassed by a caller.
- `#![deny(unsafe_op_in_unsafe_fn)]` in `weston`, and
  `clippy::undocumented_unsafe_blocks` to enforce the `// SAFETY:` rule
  mechanically rather than by the R4 audit alone.
**Decided (D15): the `pre_exec` carve-out becomes a fifth crate.** §2
currently leaves it to "decided during implementation", which breaks the
"four safe crates" invariant and forces the CI rule to carry a permanent
exception. `westonite-spawn` — unsafe allowed, one audited module — lets
the four safe crates keep an unqualified `#![forbid(unsafe_code)]` with
zero exceptions, and isolates risk R-D (fork safety: only
async-signal-safe calls between fork and exec) in a crate whose entire
contents are that audit. The C code it replaces is
`shared/process-util.c` plus the fork/exec block reached via
`wet_client_launch`/`sigchld_handler` (`main.c:367-446`). Rejected:
putting it in `weston` (mixes fork safety with the FFI wrapper — two
unrelated audit surfaces in one crate) and the `allow(unsafe_code)`
carve-out inside `westonite-shared` as the plan has it.

Note the consequence for the plan's §2 crate diagram: the workspace has
**six** crates, not five — `weston-sys`, `weston`, `westonite-spawn`
(all unsafe-bearing) and `westonite-config`, `westonite-shared`,
`westonite-shell`, `westonite` (safe). The fence rule then reads: exactly
three crates may contain `unsafe`, and the dependency check asserts the
safe four reach C only through `weston` and `westonite-spawn`.

**What validates the unsafe crates?** The plan is silent, and Miri
cannot cross FFI. **Decided (D18): sanitizers and stress tests both land
at R0/R1**, not as R4 hardening:

- An ASAN+UBSAN build running smoke + e2e — at **R0/R1**, when the
  primitives are new, not "optional at R4" (§7). This is the only tool
  that catches a wrong `container_of` offset or an early free.
- A **destroy-storm** stress test: output hotplug loops (headless/VNC),
  client kill loops, grab start/cancel races — targeted at exactly the
  invalidation paths §1/§2/§6 introduce. The e2e suite exercises
  behavior; this exercises lifetime.
- Unit tests for each primitive against a hand-built fake C object
  graph in the shim (a few `wl_signal`s and structs), so `Listener`,
  `Grab`, the registry, and the vtable trampolines are testable without
  a compositor.
- Registry validation `debug_assert`s kept on in CI builds.

## 14. Panic policy is internally inconsistent

§3(e) specifies *both* `catch_unwind` in every trampoline *and*
`panic = "abort"` in release. With `panic = "abort"`, `catch_unwind`
never runs, so the intended `weston_log` line before dying is lost —
in release, which is where it matters.

**Decided (D16): `panic = "unwind"` + `catch_unwind` at every trampoline
+ log through `weston_log` + deliberate `abort()`.** Keeps the
trampoline barrier and, more importantly, keeps the diagnostic with its
callback context in release builds. Cost: unwind tables. Rejected:
`panic = "abort"` plus a `std::panic::set_hook` logger — smaller, but the
message loses per-callback context and a panic in a `Drop` during
teardown says little.

Two things still need saying: the handler must be installed
by `wet_shell_init` as well (R1's cdylib is loaded by the C frontend,
which installs nothing), and the child side of `pre_exec` must abort
rather than unwind (§13).

## 15. R1-specific hazards worth naming in §7

R1 runs a Rust cdylib inside the C frontend, which is the phase with
the most mixed-ownership risk:

- The compositor user-data slot belongs to C (§4).
- Panic handling must be installed by the shell itself (§14).
- The temporary `config-parser.h` binding means one `weston_config *`
  is borrowed from C for the shell's lifetime — state that it is read
  once at init into `ShellConfig` and never retained, so the R3 removal
  is a deletion, not a refactor.
- `weston_desktop_api`'s `struct_size` must be set from the *bound*
  header (§3c); a shell built against different bindings than the
  frontend is exactly the ABI-drift case R-C describes, and R1 is when
  both halves coexist.

## 16. Nits and accuracy

- §3(a) says "65 uses" of the listener/`container_of` idiom. Actual
  counts in the current tree: **34** `container_of` sites (`shell.c` 21,
  `main.c` 9, `weston-screenshooter.c` 4), **32** `wl_listener` fields,
  **23** signal/destroy-listener attach sites, **6** binding
  registrations. Either fix the number or say what it counts.
- **Our own `wl_signal`**: `shell_surface` embeds
  `struct wl_signal destroy_signal` (`shell.c:89`), emitted at
  `shell.c:222`, with *our own* grabs as listeners (`shell.c:243, 301`).
  This is Rust→Rust notification routed through C machinery. Normative
  rule to add: **intra-Rust notification never goes through
  `wl_signal`.** The destroy signal disappears into the shell's own
  state (registry invalidation plus explicit fixups in the drain loop),
  removing an entire unsafe surface that a transliteration would
  recreate.
- `weston_layer` embedded by value (§3, kind 4) — no plan mention today.
- **Decided (D20): the shell reaches the wrapper through a `ShellHost`
  trait** implemented by `weston`, with a mock implementation for tests,
  so `westonite-shell`'s state machine (focus, stacking, grab state, xdg
  lifecycle) is unit-testable without a compositor. For the deep half
  that lands first (plan D4), this is the difference between testing that
  logic in `cargo test` and only ever reaching it through VNC — and it is
  the cheapest way to construct the focus-race and teardown-ordering
  cases that §1.4 says are the risky ones. Cost: a trait boundary to
  define and keep in sync with `Ctx`. Rejected: a trait covering only the
  state-machine subset (boundary judged per call site), and calling
  `weston` directly. Plan edits: §2 (crate description) and §7 (R1 exit
  criteria gain shell unit tests, not only e2e).

## 17. Decisions taken at review (2026-07-25)

Requirement agreed at review and assumed by all of these: **all
`unsafe` and all unsoundness live in one crate** (see the header). All
eight were decided by the owner; the rejected alternatives are kept so
they are not silently revisited.

| # | Question | **Decision** | Rejected |
|---|---|---|---|
| **D13** | Handle model for C-owned objects | Two handle kinds: callback **subjects** arrive as live borrows needing no check; **stored** references are generational ids (slot + generation) resolved through `weston`'s slot table — opaque, unforgeable, no pointers or bindgen types in safe APIs; registration and destroy-listener attachment paired in one function. Stale resolution → `Option`, **no `unwrap`/`expect` anywhere in the safe crates** (stricter than this review proposed). §1 | `Copy` pointer newtypes (unsound); branded lifetimes (viral); shadow state (duplicated). `Rc`-cell handles are equally sound and wrapper-internal — retained as a fallback, not a safety question. `expect`-where-provable rejected: a panic kills the session. |
| **D14** | Reentrancy strategy | Two-tier dispatch: deferrable callbacks queue events drained with no C frame live; a named, inventoried sync tier is the audited exception. Safe crates carry **no** interior mutability. §7, §8 | §3(d)'s `RefCell`-discipline-by-review; the sync-in/deferred-out middle option (leaves a per-call-site judgement about re-entrancy). |
| **D15** | `pre_exec` carve-out placement | New crate `westonite-spawn` (unsafe allowed, one audited module). Workspace becomes **six** crates; the safe four keep an unqualified `forbid(unsafe_code)` with zero exceptions. §13 | Putting it in `weston` (mixes fork safety with the FFI wrapper); an `allow(unsafe_code)` carve-out in `westonite-shared` as the plan has it. |
| **D16** | Panic policy | `panic = "unwind"` + `catch_unwind` at every trampoline + `weston_log` + deliberate `abort()`; handler also installed by `wet_shell_init`. §14 | `panic = "abort"` + `set_hook` (loses per-callback context). |
| **D17** | Fence enforcement | **All four**: dependency-graph check, public-API snapshot, opaque id types, clippy unsafe lints (`unsafe_op_in_unsafe_fn`, `undocumented_unsafe_blocks`) — plus `clippy::unwrap_used`/`expect_used` from D13. Replaces the CI grep. §13 | Adopting a subset. |
| **D18** | Wrapper validation | ASAN/UBSAN smoke+e2e **and** the destroy-storm stress test land at R0/R1, with fake-C-object unit tests per primitive and registry `debug_assert`s on in CI. §13 | Sanitizers deferred to R2; the plan's "optional at R4". |
| **D19** | Boundary cost rules | Cached strings, no per-callback `Vec`/event allocation, scope-guarded lazy log formatting — where per-event cost actually lands, as opposed to handle resolution. §10a | — |
| **D20** | Shell testability | `westonite-shell` reaches the wrapper through a `ShellHost` trait implemented by `weston`, with a mock host for unit tests. §16 | A trait covering only the state-machine subset; calling `weston` directly (e2e-only verification). |

Plan edits these imply, by section:

- **Goals** — restate goal 1 as "all `unsafe` **and all unsoundness**
  live in one crate; `weston`'s public API is sound for arbitrary safe
  callers" (§1).
- **§2** — add the dependency/public-API/opaque-id fence rules; add the
  sixth crate `westonite-spawn` to the diagram and restate the fence as
  "exactly three crates may contain `unsafe`" (D15); note that
  `westonite-shell` depends on `weston` through the `ShellHost` trait
  (D20); extend the shim's scope to variadics.
- **§3** — rewrite (b) around the two handle kinds, the slot table, and
  the cost points; add the ownership taxonomy; fix the user-data-slot
  framing; add **(f) grabs**, **(g) boundary value rules**, and
  **(h) boundary cost rules**; replace (d) with the two-tier dispatch
  and the one-borrow-at-the-edge invariant; fix (e)'s panic
  inconsistency; correct the "65 uses" count.
- **§6** — add the CI checks: dependency graph, public-API snapshot,
  clippy unsafe lints, `unwrap_used`/`expect_used`, ASAN/UBSAN job
  (D17, D18).
- **§7** — R0 exit criteria gain the callback inventory table, the
  registry/deferred-dispatch/grab primitives, the fake-C-object test
  harness, and the ASAN job; R1 gains the grab/destroy-storm stress
  cases, shell unit tests against the mock `ShellHost` (D20), and the
  hybrid hazards of §15.
- **§9** — add R-H (handle soundness) and note that deferral-induced
  ordering changes are parity bugs, not D6 divergences.
- **§10** — record D13–D20.
- **D6 (parity bar)** — note the one behavioral divergence D13 creates:
  where the C code aborts on a stale lookup (`assert(sh_output)`,
  `shell.c:266`), the Rust version logs and returns a zeroed rectangle
  (§1.6).

Reviewer's note: this document supersedes nothing in the plan on its
own — the plan's text is unchanged and still says `Copy`/`NonNull`
handles (§3b) and `RefCell`s in the shell (§3d). D13–D20 record what
those sections *should* say; applying them is a separate edit.

## 18. What this does *not* change

The plan's structure survives review: the crate split, the five
primitives as a starting set, shell-first phasing (D4), the e2e suite
as the fixed oracle, committed bindings (D5), and the §5 config
re-specification are all sound and unaffected. Every item above is an
*additional* specification inside `weston`/`weston-sys` or a CI rule —
none of it moves work into the safe crates, and most of it removes work
from them.
