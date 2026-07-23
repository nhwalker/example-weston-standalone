# Reviewer's guide: the Rust migration

Companion to `rust-migration-plan.md`. Its audience is anyone
reviewing the Rust code produced during the migration — **especially
reviewers coming from a garbage-collected language (Java, C#, Go)**
who are not yet fluent in Rust. It does not teach you to *write* Rust.
It teaches you to find the handful of places where a mistake causes
memory corruption rather than a clean exception, to check them, and
to insist they are tested in a way that would actually catch a bug.

Read `rust-migration-plan.md` §2 (crate architecture) and §3 (the
five FFI patterns) first — this guide is the "how to review it"
counterpart to those "how to build it" sections.

---

## 0. The one mental shift

In Java the garbage collector guarantees you can never touch freed
memory, and every reference is valid or `null` (and dereferencing
`null` throws a *catchable* exception). Rust gives the same
guarantees in **safe** code, but at compile time instead of via a
runtime. Use-after-free, dangling pointers, and data races are
rejected by the compiler.

The keyword **`unsafe` switches that checking off** for a block. It
does not mean "dangerous code." It means: *"compiler, I have checked
this by hand; trust me."* Inside `unsafe`, **you** are the borrow
checker. A mistake there is not an exception — it is undefined
behavior: a crash, a silent corruption, or a security hole that may
surface only under load or on another CPU.

The consequence that should drive your whole review:

> Your scrutiny is **not** spread evenly across the diff. It
> concentrates in the one crate where `unsafe` is allowed. Everything
> else you review the way you'd review Java — for logic, not for
> memory safety, because the compiler has already guaranteed the
> memory safety.

---

## 1. The map: where danger can and cannot live

From plan §2, the workspace is deliberately arranged so that all
`unsafe` lives in one small crate:

| Crate | Unsafe allowed? | How you review it |
|---|---|---|
| `weston-sys` | generated | Check it is *generated*, not hand-edited (§7). Don't read line-by-line. |
| `weston` | **yes — the only place** | **90 % of your safety scrutiny goes here.** Every `unsafe` block is a hand-written proof you are co-signing. |
| `westonite` | no (`#![forbid(unsafe_code)]`) | Like Java. Logic bugs only — cannot contain a memory-safety bug. |
| `westonite-shell` | no (forbidden) | Like Java. |
| `westonite-shared` | no (forbidden)\* | Like Java. |
| `westonite-config` | no (forbidden) | Like Java + exhaustiveness (§9). |

\* one possible carve-out for `pre_exec` fd setup — see §7.

**Your very first check on any PR:** does it add `unsafe` *outside*
the `weston` crate? If yes, the fence has a hole — a blocking finding
regardless of whether the code "works." The forbid attribute makes
this mechanical:

```rust
// Top of lib.rs in every safe crate. If this line is missing or
// was deleted in the diff, that is itself the finding.
#![forbid(unsafe_code)]
```

`forbid` (unlike `deny`) cannot be locally overridden with an
`#[allow(...)]`, so its presence is a hard guarantee for the whole
crate. Grep for it; CI enforces it (plan §2).

---

## 2. The five FFI patterns — spot / check / example

These come from plan §3. For each: what actually goes wrong, the
check that catches it, and good-vs-bad code.

### 2a. Listener trampolines (`wl_listener` + `container_of`)

libweston notifies us by calling a C function pointer we registered.
That function receives a pointer *into the middle of* our struct and
must recover the whole struct (the C `container_of` idiom). Two ways
it kills you:

- **(a)** the struct was already dropped → the callback touches freed
  memory (**use-after-free**);
- **(b)** the struct moved in memory → the registered pointer is
  stale.

The safety contract that prevents both: the listener object is
**pinned** (cannot move) and its `Drop` **removes itself** from
libweston's list (cannot fire after death).

```rust
// GOOD — the Listener primitive owns a pinned box and unregisters on Drop.
struct Listener {
    inner: Pin<Box<Inner>>,
}
struct Inner {
    listener: wl_listener,          // the C struct libweston stores a pointer to
    callback: Box<dyn FnMut(*mut c_void)>,
    _pin: PhantomPinned,            // makes Inner !Unpin: it cannot be moved out
}

extern "C" fn trampoline(l: *mut wl_listener, data: *mut c_void) {
    // SAFETY: `l` points at the `listener` field of an `Inner` that
    // is still alive: it is only unregistered in Inner's Drop, which
    // runs wl_list_remove before the box is freed, so libweston can
    // never call this after the box dies. container_of recovers Inner.
    let inner = unsafe { container_of!(l, Inner, listener) };
    (unsafe { &mut *inner }.callback)(data);
}

impl Drop for Inner {
    fn drop(&mut self) {
        // SAFETY: removes our node from libweston's signal list so the
        // trampoline can never fire against this freed allocation.
        unsafe { wl_list_remove(&mut self.listener.link); }
    }
}
```

```rust
// BAD — no Drop removes the listener. The moment `Listener` is dropped,
// libweston still holds a pointer to freed memory and will call the
// trampoline on the next signal → use-after-free.
struct Listener { inner: Box<Inner> }   // not pinned, and no Drop impl
```

**Review checks:**
- Is there a `Drop` that calls `wl_list_remove` (or equivalent)?
- Is the object pinned / `!Unpin` so the registered address stays
  valid? (Look for `Pin<Box<...>>`, `PhantomPinned`.)
- Does application code (in `westonite-shell`) use this primitive
  rather than doing its own `container_of`? Raw `container_of`
  outside `weston` is a smell.

### 2b. C-owned object handles — **use-after-free, the #1 risk**

libweston owns `weston_output`, `weston_surface`, `weston_seat`, …
and frees them on *its* schedule, announcing death via a destroy
signal. We hold raw-pointer handles. The danger: **using a handle
after libweston freed the object.**

```rust
// A handle is a thin, Copy wrapper around a pointer. Holding one is
// fine; the danger is holding it PAST the object's destruction.
#[derive(Clone, Copy)]
struct Output(NonNull<weston_output>);
```

The rule: **any handle stored beyond the current call stack must be
paired with a destroy listener that removes it.** Transient use
(receive handle in a callback, use it, return) is fine.

```rust
// GOOD — our per-output state is torn down when libweston destroys
// the output. After this fires, nothing holds the stale pointer.
fn track_output(shell: &Rc<Shell>, output: Output) {
    let listener = Listener::once(move || {
        shell.outputs.borrow_mut().remove(&output.as_ptr());
    });
    output.add_destroy_listener(listener);
    shell.outputs.borrow_mut().insert(output.as_ptr(), OutputState::new(listener));
}
```

```rust
// BAD — the handle is cached forever. When libweston destroys the
// output, `self.outputs` still contains a dangling pointer; the next
// repaint/iteration dereferences freed memory.
fn track_output(&self, output: Output) {
    self.outputs.borrow_mut().push(output);   // nothing removes it on destroy
}
```

**Review check — the single most valuable question in the whole
review:** for every handle cached in a struct field, ask *"what
removes this when libweston frees the object?"* If the answer is not
in the code, that is a use-after-free waiting to happen. Ordinary
tests may not reveal it (see §5).

### 2c. C vtables / trampolines (`weston_desktop_api`, backend configs)

The shell implements a Rust trait; static `extern "C"` shims fill the
C function-pointer table libweston calls.

```rust
// The safe surface: a Rust trait the shell implements.
trait DesktopApi {
    fn surface_added(&self, surface: DesktopSurface);
    fn surface_removed(&self, surface: DesktopSurface);
    // ...
}

// The unsafe bridge, entirely inside `weston`. One shim per entry.
extern "C" fn surface_added_shim(surf: *mut weston_desktop_surface,
                                 user_data: *mut c_void) {
    ffi_guard(|| {                              // catch_unwind — see §2e
        // SAFETY: libweston passes the `user_data` we registered in
        // weston_desktop_create; it is a live `*const ShellImpl` for
        // the compositor's whole lifetime (destroyed last, in shutdown).
        let shell = unsafe { &*(user_data as *const ShellImpl) };
        shell.surface_added(DesktopSurface::from_ptr(surf));
    });
}

static API: weston_desktop_api = weston_desktop_api {
    struct_size: size_of::<weston_desktop_api>(),   // ← see §3, ABI
    surface_added: Some(surface_added_shim),
    surface_removed: Some(surface_removed_shim),
    // ...
};
```

**Review checks:** every shim is wrapped in a panic guard (§2e); the
`user_data` cast matches what was actually registered; `struct_size`
is set (§3).

### 2d. Reentrancy and `RefCell` — the runtime borrow check

Because the compositor is single-threaded, mutable shared state lives
behind `RefCell`, which enforces "one mutable borrow at a time" **at
runtime** (a `RefCell` is roughly a non-blocking read/write lock with
no actual locking — it just panics instead of blocking). If we hold a
mutable borrow and call into libweston, and libweston synchronously
calls *back* into our code, the second borrow panics with
`BorrowMutError`.

This is the design working as intended: it converts what would be C
memory-corruption into a **deterministic, safe panic**. But a panic
in production is still a bug you want caught in review.

```rust
// BAD — borrow held across a call back into libweston. If activating
// the surface makes libweston synchronously emit a signal that re-enters
// `focus()` (very common during focus changes), the reentrant
// borrow_mut() panics with BorrowMutError.
fn focus(&self, surface: DesktopSurface) {
    let mut state = self.state.borrow_mut();       // borrow starts
    state.focused = Some(surface);
    surface.activate();                            // ← FFI call, may re-enter
    state.last_activated = now();                  // borrow still held here
}                                                  // borrow ends
```

```rust
// GOOD — mutate, drop the borrow, THEN call libweston. The FFI call
// runs with no outstanding borrow, so reentrancy is fine.
fn focus(&self, surface: DesktopSurface) {
    {
        let mut state = self.state.borrow_mut();
        state.focused = Some(surface);
        state.last_activated = now();
    }                                              // borrow dropped here
    surface.activate();                            // safe to re-enter now
}
```

**Review check:** scan every `borrow_mut()` (and `borrow()`) and ask
*does this borrow live across a call into libweston?* If a `.borrow*()`
result — including one bound to a `let` — is still in scope when an
FFI method is called, that is the reentrancy smell. The fix is
always to narrow the borrow scope.

### 2e. Panics must not cross into C

In Java an exception unwinds until something catches it. A Rust panic
unwinding **into C** is undefined behavior. Every function libweston
can call must catch the panic and turn it into a logged `abort()`.

```rust
// The guard used by every extern "C" shim in `weston`.
fn ffi_guard(f: impl FnOnce()) {
    // catch_unwind stops a panic at the FFI boundary. On panic we log
    // and abort — a clean, single crash instead of UB in the C stack.
    if std::panic::catch_unwind(AssertUnwindSafe(f)).is_err() {
        weston_log("panic in Rust callback; aborting");
        std::process::abort();
    }
}
```

```rust
// BAD — a shim with no guard. Any panic inside (an unwrap, an
// out-of-bounds index, a BorrowMutError from §2d) unwinds straight
// into libweston's C frames → undefined behavior.
extern "C" fn output_destroyed_shim(o: *mut weston_output, data: *mut c_void) {
    let shell = unsafe { &*(data as *const ShellImpl) };
    shell.output_destroyed(Output::from_ptr(o));   // may panic → UB
}
```

**Review checks:** every `extern "C"` function begins with the guard;
the workspace sets `panic = "abort"` in release profiles as a
backstop (plan §3e). Both together — belt and suspenders.

---

## 3. ABI versioning (smaller, still sharp)

Many libweston structs carry `struct_size` / `version` fields so the
library can evolve without breaking callers. If a builder forgets to
set them, libweston reads uninitialized bytes as though they were
real fields.

```rust
// GOOD — size/version set from the bound headers.
let config = weston_headless_backend_config {
    base: weston_backend_config {
        struct_version: WESTON_HEADLESS_BACKEND_CONFIG_VERSION,
        struct_size: size_of::<weston_headless_backend_config>(),
    },
    // ... real fields ...
};
```

```rust
// BAD — zeroed base. libweston branches on struct_version/size and
// will misread the struct (or reject it) — a latent ABI landmine that
// "works" until the next libweston bump.
let config = weston_headless_backend_config {
    base: unsafe { std::mem::zeroed() },
    // ...
};
```

**Review check:** every `weston_*_backend_config` (and any struct
with `struct_size`/`version` fields) sets them from the bound
constants — never `zeroed()`, never `0`. Tie-in: plan §9 R-C and the
pkg-config version tripwire (§6) guard the header-drift direction.

---

## 4. Fork safety (`pre_exec`)

The process-spawn path (porting `process-util.c` / the fork-exec in
`main.c`) runs Rust code **after `fork()` and before `exec()`**. In
that window only **async-signal-safe** operations are legal: raw
syscalls, essentially. **No memory allocation, no locks, none of the
usual standard library** — the child shares the parent's address
space in a frozen state and allocating or locking can deadlock or
corrupt it.

```rust
// GOOD — between fork and exec, only fd syscalls. Everything that
// needs allocation (building argv, env) was done BEFORE the fork.
unsafe {
    command.pre_exec(|| {
        // SAFETY: async-signal-safe only. rustix dup2/fcntl are raw
        // syscalls; no allocation, no locking here.
        for fd in &fds_to_keep {
            rustix::io::dup2(fd, ...)?;      // clear CLOEXEC etc.
        }
        Ok(())
    });
}
```

```rust
// BAD — allocates and formats inside pre_exec. `format!` allocates;
// if the parent held the allocator lock at fork time, this can
// deadlock the child. Also touches a Vec that may reallocate.
unsafe {
    command.pre_exec(|| {
        let msg = format!("starting child {}", name);   // allocation → forbidden
        log_buffer.push(msg);                           // Vec growth → forbidden
        Ok(())
    });
}
```

**Review check:** the closure passed to `pre_exec` contains only raw
fd/syscall operations. Any `String`, `Vec` growth, `format!`,
`println!`, `Box`, or `.to_owned()` in that window is a bug. This is
the one place `unsafe` may appear outside `weston` (plan §2) — review
it with the same intensity.

---

## 5. How to verify safety is actually *tested*

This is the part that most surprises reviewers from a GC background:

> **Memory-safety bugs frequently pass ordinary tests.** Undefined
> behavior is *allowed* to look correct. A use-after-free usually
> reads the right value — right up until the allocator reuses the
> freed slot, which may be never in a short test and always in
> production.

So "the tests are green" is far weaker evidence of *safety* than it
is of correct business logic. Demand more:

**5a. Sanitizers, not just tests.** Ask whether CI runs the smoke +
e2e suites under **AddressSanitizer** for the `weston`-exercising
paths. ASan turns silent use-after-free / buffer-overflow / leak into
a loud failure *at the moment it happens*.

```
# The kind of invocation you want to see in CI for the unsafe crate's tests.
RUSTFLAGS="-Zsanitizer=address" cargo +nightly test -p weston --target x86_64-unknown-linux-gnu
```

Also **Miri** (`cargo +nightly miri test`), which detects UB in pure
Rust — but know its limit: **Miri cannot cross the FFI boundary into
libweston.** It validates the Rust-side pointer logic (offsets,
aliasing, `container_of`) but not the full round-trip. Necessary, not
sufficient.

**5b. Teardown / reentrancy coverage, not happy paths.** The
use-after-free and reentrancy bugs live in *destruction ordering*, so
the tests that actually exercise the danger are the ugly ones:

- output hotplug **removal** (not just add);
- a client disconnecting **mid-move/resize-grab**;
- compositor **shutdown with live windows**;
- focus churning between rapidly created-and-destroyed surfaces.

The plan leans on the e2e window suite for exactly this (plan §7 R1,
§9 R-A). Per unsafe primitive, the check is:

> Is there a test that drives its **`Drop` path and its destroy-signal
> path**, not just its construction?

A listener primitive tested only by "it fires when signaled" is
half-tested. You also need "it does **not** fire after the owner is
dropped." A unit test can assert the negative directly:

```rust
#[test]
fn listener_does_not_fire_after_drop() {
    let signal = TestSignal::new();
    let fired = Rc::new(Cell::new(false));
    {
        let f = fired.clone();
        let _l = signal.add_listener(move || f.set(true));
        signal.emit();
        assert!(fired.get());              // fires while alive
        fired.set(false);
    }                                      // listener dropped here
    signal.emit();                         // must NOT fire now
    assert!(!fired.get(), "listener fired after being dropped (use-after-free)");
}
```

**5c. Different kind of check for the config work.** For
`westonite-config`, the risk isn't memory — it's silently dropping an
option (plan §9 R-G). The check there is exhaustiveness: the
mapping-checklist (every capability-doc key → a `Config` field) and
`#[serde(deny_unknown_fields)]` catching the reverse (phantom keys).
Reviewing that is closer to a spec audit than a safety audit.

---

## 6. Reading `SAFETY:` comments critically

The plan requires a `// SAFETY:` comment on every `unsafe` block.
Treat each as a **claim to be falsified**, not documentation to skim.

- A **bad** SAFETY comment restates the code:
  `// SAFETY: we call the C function.` Useless — reject it in review.
- A **good** one names the invariant and *why it holds here*:
  `// SAFETY: ptr came from weston_output_create (non-null) and we
  hold it only until the destroy listener below fires, so it cannot
  dangle.`

Your move for each block: ask **"what would make this false?"** and
check the surrounding code prevents it. If you can construct a
sequence of events that violates the stated invariant — a destroy
firing first, a reentrant call, a moved object — you have found a
real bug, and it is exactly the kind ordinary tests miss.

---

## 7. What you can (mostly) *not* worry about

So you spend scrutiny where it counts:

- **Data races / lock discipline** — off the table. The code is
  single-threaded and the wrapper types are `!Send`/`!Sync` (plan
  §3d), so the compiler forbids sharing across threads. You won't be
  reviewing concurrency.
- **Buffer overflows / array bounds** in safe code — impossible; the
  compiler bounds-checks every index.
- **Leaks** — a bug, but **not** a memory-safety bug in Rust (leaking
  is safe). Don't let a leak block a PR the way a use-after-free
  would; file it, don't gate on it.
- **Integer overflow** — panics in debug builds, wraps in release;
  minor.
- **A sea of `Option`/`Result`/`?`/`match`** — that's just the
  language's null-safety and error handling, the direct analog of
  Java's checked exceptions plus `Optional`. One nit worth a glance:
  `.unwrap()` on a pointer returned from libweston (libweston *can*
  return null, and `unwrap()` on `None` panics) — a robustness nit,
  not a corruption risk.

---

## 8. One-page review checklist

For any PR in this migration:

- [ ] Does it add `unsafe` **outside** the `weston` crate (excepting
      the audited `pre_exec`, §4)? → **blocking.**
- [ ] Do the four safe crates still carry `#![forbid(unsafe_code)]`?
      → grep it (§1).
- [ ] For each new `unsafe` block: is the `SAFETY:` comment a real,
      **falsifiable** argument? Can I construct a sequence that breaks
      it? (§6)
- [ ] For each C-object handle stored in a field: **what invalidates
      it on destroy?** (§2b — the highest-value question.)
- [ ] For each listener / trampoline: does `Drop` remove it? Is it
      pinned? Is it panic-guarded? (§2a, §2e)
- [ ] Any `borrow*()` held **across** a call into libweston? →
      reentrancy smell. (§2d)
- [ ] Every `extern "C"` function guarded by `catch_unwind`? (§2e)
- [ ] Every `struct_size`/`version` field set from bound constants,
      never `zeroed()`? (§3)
- [ ] Anything but raw syscalls between fork and exec? (§4)
- [ ] New unsafe primitive: is its **teardown / destroy** path tested,
      not just construction? (§5b)
- [ ] Do the `weston`-exercising tests run under **ASan / Miri** in
      CI? (§5a)
- [ ] (config PRs) Every option accounted for in the R-G checklist,
      `deny_unknown_fields` on? (§5c)

If you internalize just three of these — *no unsafe outside the
fence*, *what invalidates each handle on destroy*, and *is the
teardown path tested* — you will catch the failures that actually
matter.
