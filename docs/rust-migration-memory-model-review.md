# Review: memory-model fencing in the Rust migration plan

Status: **review notes, not yet folded into the plan**. Reviews
`docs/rust-migration-plan.md` (§2, §3, §6, §9) against the actual C
sources in this repo and the installed libweston 14 API, with one
question in focus: *does the plan, as written, guarantee that all
complex memory models stay inside `weston-sys`/`weston`, and that the
four safe crates get the most straightforward model available?*

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
can be made without re-deriving. Sections that need an owner decision
are marked **(needs D-number)**; §17 collects them.

## 1. The core soundness gap: `Copy` pointer handles

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

The C code *does* let objects die under us, and it *does* carry the
resulting invalidation by hand:

- `destroy_shell_grab_shsurf()` nulls a back-pointer on death —
  `desktop-shell/shell.c:178`.
- `desktop_surface_removed()` nulls `shseat->focused_surface` on every
  seat, with a comment explaining the stale-pointer hazard —
  `desktop-shell/shell.c:1263-1291`.
- `shsurf->desktop_surface = NULL` leaves a *half-dead* shell surface
  that is still used afterwards — `desktop-shell/shell.c:1290`.

A safe API that can be handed a dangling handle is a broken fence, no
matter how clean `weston`'s internals are. Rust does not need to
reproduce this manual nulling — but it does need one mechanism.

**Proposal (needs D13): generational handles resolved through a
registry owned by `weston`.**

- `weston` keeps one registry per object kind: `SlotMap<Key, NonNull<T>>`
  (or `HashMap<*mut T, (Key, Generation)>` for the reverse direction,
  needed when a callback hands us a pointer).
- Safe crates receive `OutputId`, `SurfaceId`, `ViewId`, `SeatId`,
  `HeadId` — `Copy + Eq + Ord + Hash + Debug`, containing an index and
  a generation, **no pointer**.
- A destroy listener is attached the moment an object first crosses the
  fence; its trampoline bumps the generation and drops the slot.
- Every accessor resolves the id first and returns
  `Option<…>`/`Result<…, Dead>`. A stale id is `None` — a deterministic,
  loggable, testable outcome, never UB.

Consequences worth stating in the plan, because they are the *dividend*
the safe crates get:

- Safe state can hold ids for as long as it likes — including in
  `HashMap` keys and in `Option<SurfaceId>` back-references — with no
  lifetimes, no `Rc`, no `Weak`, no cycles, and no `Drop` ordering
  puzzles. This is the "most straightforward memory model" the goals
  ask for: **ids into a map**.
- `find_shell_output_from_weston_output()`
  (`desktop-shell/shell.c:163-175`) and `get_shell_seat()`
  (`desktop-shell/shell.c:1179-1193`) become ordinary safe map lookups.
- Derived `Eq`/`Hash` on ids is safe; derived `Eq`/`Hash` on pointer
  newtypes invites comparing a dead pointer to a recycled live one
  (allocators reuse addresses — a real false-positive source).

Cost: one lookup per API call. At libweston's call rates (input events,
commits) this is noise; say so explicitly so it does not get
re-litigated during implementation. If a hot path ever shows up, the
escape hatch is a `&Ctx`-scoped resolved borrow (§7), not a raw handle
in safe state.

Alternative considered: `Rc<ObjectRef>`/`Weak` with a dead-flag.
Equivalent soundness, but it puts refcount plumbing and `upgrade()`
noise into every safe call site, and `Rc` cycles between shell state
and per-object state are easy to build by accident. Recommend the
registry; record the rejection so it is not revisited.

Add as **R-H** to §9: *"`Copy` pointer handles would make the safe
crates capable of UB; handles are generational ids resolved through the
wrapper's registry."*

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

Proposal (needs D14): **two-tier dispatch**, with each callback
classified in the plan.

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
where it matters. Stronger, cheap, and mechanical:

- **Dependency-graph rule**: the four safe crates must not depend on
  `weston-sys` at all (only on `weston`). Checked with `cargo metadata`
  in CI. This is the rule that actually prevents "safe" code from
  holding raw pointers, and it is unforgeable — unlike a grep.
- **Public-API rule**: `weston`'s public API contains no raw pointers,
  no `NonNull`, and no `weston_sys::*` types. Enforce with a
  `cargo public-api` snapshot test (also gives a reviewable diff every
  time the fence moves).
- `#![deny(unsafe_op_in_unsafe_fn)]` in `weston`, and
  `clippy::undocumented_unsafe_blocks` to enforce the `// SAFETY:` rule
  mechanically rather than by the R4 audit alone.
- **Resolve the `pre_exec` carve-out now (needs D15).** §2 leaves it to
  "decided during implementation", which breaks the "four safe crates"
  invariant and forces the CI rule to carry an exception.
  Recommendation: a fifth small crate (`westonite-spawn`, unsafe
  allowed, audited, one module) so the four remain unqualified. It also
  isolates R-D (fork-safety: only async-signal-safe calls between fork
  and exec) in a crate whose entire contents are that audit —
  the relevant C code is `shared/process-util.c` and the fork/exec block
  reached via `wet_client_launch`/`sigchld_handler` (`main.c:367-446`).

**What validates the unsafe crate?** The plan is silent, and Miri
cannot cross FFI. Add:

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

Pick one (needs D16):

- **`panic = "unwind"` + `catch_unwind` at every trampoline + log +
  `abort()`** (recommended): keeps the trampoline barrier and the log
  line; costs unwind tables.
- **`panic = "abort"` + `std::panic::set_hook`** that logs through
  `weston_log`: cheaper, but no per-trampoline context, and a panic in
  a `Drop` during teardown gives a less useful message.

Either way, two things need saying: the hook/handler must be installed
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
- Consider defining the shell's dependency on the wrapper as a
  **trait** (`ShellHost`) implemented by `weston`, so `westonite-shell`
  is unit-testable against a mock host without libweston. For the
  "deep half" that lands first (D4), that is the difference between
  testing focus/stacking logic in `cargo test` and only being able to
  test it through VNC.

## 17. Proposed decisions

| # | Question | Recommendation |
|---|---|---|
| **D13** | Handle model for C-owned objects | Generational ids resolved through a registry in `weston`; no pointers, no `NonNull`, no bindgen types in safe APIs. Reject `Copy` pointer newtypes (unsound) and `Rc`/`Weak` (plumbing, cycles). §1 |
| **D14** | Reentrancy strategy | Two-tier dispatch: deferrable callbacks queue events drained with no C frame live; a named, inventoried sync tier is the audited exception. Safe crates carry **no** interior mutability. §7, §8 |
| **D15** | `pre_exec` carve-out placement | Fifth crate `westonite-spawn` (unsafe allowed, one audited module); the four safe crates keep an unqualified `forbid(unsafe_code)`. §13 |
| **D16** | Panic policy | `panic = "unwind"` + `catch_unwind` + log + `abort()` at every trampoline; hook installed by `wet_shell_init` too. §14 |
| **D17** | Fence enforcement | Dependency-graph check + public-API snapshot + `clippy::undocumented_unsafe_blocks`, replacing the CI grep. §13 |
| **D18** | Wrapper validation | ASAN/UBSAN smoke+e2e job and a destroy-storm stress test from R0/R1 (not optional at R4); fake-C-object unit tests for each primitive. §13 |

Plan edits these imply, by section:

- **§2** — add the dependency/public-API fence rules; resolve the
  `pre_exec` carve-out (D15); extend the shim's scope to variadics.
- **§3** — rewrite (b) around the registry and the ownership taxonomy;
  fix the user-data-slot framing; add **(f) grabs** and
  **(g) boundary value rules**; replace (d) with the two-tier dispatch
  and the one-borrow-at-the-edge invariant; fix (e)'s panic
  inconsistency; correct the "65 uses" count.
- **§6** — add the CI checks (dep graph, public API, ASAN job, clippy
  SAFETY lint).
- **§7** — R0 exit criteria gain the callback inventory table, the
  registry/deferred-dispatch/grab primitives, the fake-C-object test
  harness, and the ASAN job; R1 gains the grab/destroy-storm stress
  cases and the hybrid hazards of §15.
- **§9** — add R-H (handle soundness) and note that deferral-induced
  ordering changes are parity bugs, not D6 divergences.
- **§10** — record D13–D18.

## 18. What this does *not* change

The plan's structure survives review: the crate split, the five
primitives as a starting set, shell-first phasing (D4), the e2e suite
as the fixed oracle, committed bindings (D5), and the §5 config
re-specification are all sound and unaffected. Every item above is an
*additional* specification inside `weston`/`weston-sys` or a CI rule —
none of it moves work into the safe crates, and most of it removes work
from them.
