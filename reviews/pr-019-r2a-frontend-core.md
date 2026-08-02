# Review: PR #19 — rust(R2a): Rust frontend core — §5 config interface, spawn, headless

| | |
|---|---|
| **PR** | [#19](https://github.com/nhwalker/example-weston-standalone/pull/19) "rust(R2a): Rust frontend core — §5 config interface, spawn, headless" |
| **Merge commit** | `3e65c95` (base `97b4161`, head `30edc36`) |
| **Merged** | 2026-07-26 |
| **Diff size** | 30 files, +3,204 / −60 |
| **C baseline** | `frontend/main.c` (config/CLI/signal/autolaunch paths), `shared/process-util.c` |
| **Reviewed** | all of `westonite-config` (model/cli/overrides/resolve), `westonite-spawn`, `westonite/main.rs`, the `weston` crate diffs (builder, signals, log sink, `attach_shell_native`), scripts/harness/CI diffs; autolaunch + sigchld semantics verified against `main.c` |

## What the PR does

The Rust frontend exists: `westonite-rs`, with the §5 re-specified
config interface (serde `Config` over the full ini surface, kebab-case,
`deny_unknown_fields`, clap CLI, `-o` dotted overrides, XDG discovery
of `westonite.toml` with the legacy-ini hint), a `--log` file sink, the
`verify_xdg_runtime_dir` port, headless bring-up through the fence
builder with the statically linked Rust shell, autolaunch through the
new audited-unsafe `westonite-spawn` crate, and a SIGCHLD watch. A
fail-loud gate refuses everything not yet ported. The e2e harness gains
a TOML mode + ini→TOML translation so behavior tests run unmodified
against both frontends.

## Verdict

The config re-spec is the strongest sub-system: precedence
(defaults → file → `-o` → flags) is implemented once, unit-tested from
every direction (including message-wording pins against the C
frontend), and `-o` overrides patch the TOML tree *before*
deserialization so they get identical typing/unknown-key treatment —
with a test asserting a rejected override leaves the tree untouched.
`westonite-spawn`'s `pre_exec` audit is disciplined and its two
narrowings of C's exec-string grammar are documented *as divergences*
with tests, not passed off as parity.

The two significant problems are (1) a latent major bug in how the
shell meets the frontend's registries — invisible to every test this PR
runs — and (2) the config surface audit (risk R-G) shipping with holes
that later slices kept discovering one at a time.

## Findings — correctness

### PR19-C1 (major, latent): under `westonite-rs` the shell never receives `OutputCreated` — no background curtain is ever created — **fixed in #20**

The chain, all verifiable at this merge:

1. `build()` attaches the shell *before* the heads flush
   (`compositor.rs:225-230`), so the initial output arrives via the
   shell's `output_created` listener rather than C's existing-outputs
   iteration.
2. The R0 canned `enable_head` still registers the output in the
   frontend's registry **before** `weston_output_enable`
   (`compositor.rs`, unchanged from PR #16 — see PR16-C3).
3. When `output_created_signal` then fires, the shell path's
   `register_output_shell` hits its `id_of(output).is_some()` early
   return (`shell_init.rs:270-272` at this revision) — placed there to
   make registration idempotent — and returns **before** dispatching
   `Event::OutputCreated`.

`OutputCreated` is what creates the background curtain, so the Rust
frontend ran with no background at all. The hybrid path (C frontend +
Rust shell) was immune because C never calls the frontend-side
`register_output`. Nothing in this PR's validation could see it: the
R2a e2e subset (`test_cli`/`test_lifecycle`/`test_children`) and both
new smoke legs assert sockets, log markers, and exit codes — no pixel
or protocol-level output existence check. #20's review round diagnosed
exactly this and moved frontend registration after
`weston_output_enable`.

The meta-lesson is worth recording: "27 passed" against a subset chosen
for what's *portable* is not evidence about what's *visible*. The first
pixel-level check (VNC framebuffer at R2c) validated the fix, not the
original.

### PR19-C2 (moderate divergence): the signal-source port made the R0 signal shape permanent — still live on main

R2a is the slice that owns C's `signals[]` block (`main.c:4544-4576`),
and it ported it as: SIGTERM + SIGINT + SIGCHLD via
`wl_event_loop_add_signal`. C's actual shape: SIGTERM + **SIGUSR2** via
the loop, SIGINT via plain `sigaction` whose handler `raise(SIGUSR2)`s
(deliberately, so gdb can catch Ctrl+C — the comment block explains
it), SIGCHLD via the loop, SIGUSR1 blocked process-wide. Consequences
carried forward from PR16-C4, now attributable to this slice: `kill
-USR2` does not terminate `westonite-rs` cleanly; Ctrl+C under gdb
exits the compositor instead of breaking; no `caught signal %d` log
line. None of this is in the PR's divergence notes. Verified still
present on current `main`.

Also introduced here: `on_sigchld` and `on_term_signal` run as bare
callbacks outside `with_depth`, creating the "bare-bodied event-loop
signal callbacks" dispatch blind spot that #26's audit later wrapped —
at this merge, any deferred event enqueued from the SIGCHLD path had
no drain edge of its own. **Wrap added in #26.**

### PR19-C3 (moderate): `--backend`/`--backends` modeled as two prioritized fields; C has one variable — **fixed in #22**

`resolve.rs:268-280`: `cli.backend` wins over `cli.backends`
unconditionally; `cli.backend` is never comma-split; and on the config
side `core.backend` is read *before* `core.backends`. C
(`main.c:4458-4459, 4602-4606`) treats both flags as one variable
(last occurrence wins, either accepts a comma list) and reads
`backends=` first with `backend=` as the fallback — the Rust version
inverts both. Broke the e2e multi-backend spelling
`--backend=vnc --backends=headless,vnc` at R2c, where #22 collapsed it
to one clap field with a visible alias.

### PR19-C4 (moderate): the config-surface sweep (risk R-G) shipped incomplete in both directions — closed piecemeal across #20–#36

Modeled-but-fictional keys (C never reads them; accepted here, so
`deny_unknown_fields` blessed config that libweston cannot honor):

* `[[output]] vrr-mode` — no reader anywhere in 14.0.1; **removed #33**
* `[[color-characteristics]] max-cll` — no such field in
  `weston_color_characteristics`; **removed #33**

Real keys/flags C reads that the model/CLI lacked (each one silently
diverging or erroring until its slice noticed):

* `--transform` (headless CLI option) — **added #20**
* `[core] repaint-window` — **added #22**
* `--address` / `[vnc] address` — **added #22**
* RDP flag drift: invented `--no-clients-resize` vs C's
  `--no-resizeable`; missing `--rdp4-key`/`--env-socket`/
  `--no-remotefx-codec` — **fixed #23**
* `[output] allow-hdcp` — **added #33**
* `[libinput] disable-while-typing` — **added #34/#35**
* `[core] wait-for-debugger` — **added #35**
* `[core] output-decorations`, `use-gl`, `use-pixman` — **added #36**

No single one is damning; the pattern is: the PR description claims
"serde `Config` over the full ini surface", and the mechanical
key-by-key diff against `weston_config_section_get_*` that would have
made that true was only done at #36 (which found the last three). Doing
that audit *here* was cheaper than doing it seven times.

### PR19-C5 (minor divergence): `parse_mode` trims where C's `sscanf` rejects — **fixed in #20**

`main.rs:289-295`: `"1024 x 640"` parses (each side is trimmed); C's
`sscanf(mode, "%dx%d", …)` fails on the interior space and falls back
to defaults with the "Invalid mode" log. #20 removed the trim. The
other half — Rust rejecting non-positive dimensions that C would pass
through to the backend — is a safer-side divergence that stayed, and
deserves the one-line comment it never got.

### PR19-C6 (minor divergence, undocumented): color parsing — C is always base-16, the Rust model is base-10 unless `0x`-prefixed — **fixed in #42**

`resolve.rs:160-168` vs `weston_config_section_get_color`
(`strtoul(value, &end, 16)`): an ini `background-color=ff002244`
(legal and common in weston configs, no prefix) fails the Rust parser
as "invalid color"; a bare digit string is decimal here, hex in C. The
`0x` spelling behaves identically in both, and the harness only emits
that spelling — but `docs/config-migration.md` doesn't mention that
unprefixed hex, the C default notation, does not survive the
migration. Was still live on main as far as this review traced it;
**fixed in #42** (C's always-base-16 grammar restored, with the
bare-TOML-integer override path preserved by value, unit-tested, and
the migration doc updated).

### PR19-C7 (minor divergence): the Rust frontend's log lines have no timestamps — **fixed in #35 (R2f)**

`log.rs`: the `--log` sink writes the shim-formatted line straight to
the file. The C frontend's `vlog` prepends `weston_log_timestamp()`
(date/time + pid). Every Rust-frontend log line therefore lacked the
timestamp column — a plainly visible format divergence that survived
five slices because every e2e assertion is an `re.search`. R2f's
PROVENANCE entry names this exact gap ("a divergence nobody had
noticed") when rerouting logging through the weston-log scope stack.

### PR19-C8 (minor divergence, documented rationale): shell attach point vs C

`with_shell` attaches the shell after `backends_loaded`, before socket
+ flush; C loads the shell after flush and socket
(`main.c:4671→4706→4714`). The in-code comment gives the rationale
(shell listeners see every output creation). Consequences: initial
outputs take the hotplug path under the Rust frontend (a C shell only
does this for genuinely hotplugged outputs), which is precisely the
path C1 broke; and the PR16-C6 socket-before-flush ordering persists.
Reasonable choice — but it's an event-sequencing divergence and only
the motivation, not the divergence, is documented.

### PR19-C9 (nit): one `rdp_vnc_port` field for two backends

`resolve.rs:363-370` picks VNC's section over RDP's when both backends
are requested; C's loaders each read their own section, so
`--backends=rdp,vnc` with both ports set configures both in C and only
VNC here. Academic at R2a (both refused), semi-academic later (RDP
dropped in #27), but the field name advertises the smell.

### PR19-C10 (nit): `Settings.log_file` is dead — **fixed in #49**

`main.rs` opens the log from `cli.log` before resolution (correctly —
config errors must reach the sink); `Settings.log_file` is populated
and never read. Either drop the field or note it's reserved for R2f.

### PR19-C11 (nit, documented): autolaunch exec semantics

C `execute_autolaunch` uses `access(path, X_OK)` + `execl` (no PATH
search, no arguments); Rust prechecks with a metadata approximation
(any execute bit — documented in-code) and spawns with PATH lookup.
For the config path this widens what works (`path = "firefox"`
resolves via PATH in Rust, fails in C). Harmless-to-useful, but it's a
behavior difference in a documented config key, not recorded in
`config-migration.md`.

## Behavioral divergences — deliberate & documented (sound)

* The §5 re-spec itself: TOML instead of ini, `deny_unknown_fields`
  turning silent typos into startup errors (D9), `-o` overrides (D10),
  no `WESTON_CONFIG_FILE` export (D12), legacy-ini hint (D11),
  XDG_CONFIG_DIRS entries used directly instead of C's hard-coded
  `weston/` subdirectory — all documented in
  `docs/config-migration.md`.
* Fail-loud gate: unported backends/options are startup errors, with
  exactly two named warn-and-ignore exceptions (`--logger-scopes`/
  `--flight-rec-scopes`, `--wait-for-debugger`) — both later closed by
  R2f. The right shape for an incremental port.
* `from_exec_string`'s two narrowings of C's `VAR=value` grammar
  (path-with-`=`, empty key) — documented + tested.
* XDG_RUNTIME_DIR check: C parity restored during the PR's own review
  round (0777 mask, owner check), with `getuid` routed through the
  audited crate so the frontend keeps `forbid(unsafe_code)` — a nice
  fence-discipline touch.

## Findings — style

### PR19-S1: the `hybrid-r1` feature now gates non-hybrid code

`with_shell`/`attach_shell_native` — the *native* Rust-frontend shell
attach — are `#[cfg(feature = "hybrid-r1")]`. The feature name
promises "dies at R3", but the native attach is the R3 survivor. Either
rename the feature or split the gates; today a reader auditing "what
disappears at R3" gets the wrong answer from the cfg.

### PR19-S2: the compositor-destroy teardown closure is duplicated

`shell_init_body` and `attach_shell_native` each contain an identical
9-line destroy-listener body (Shutdown dispatch → curtains → desktop →
layers → seat recs → teardown). `wire_common` was extracted for the
shared wiring; the teardown twin was left duplicated, and a future
teardown-ordering fix now has two places to miss.

## Positive notes

* `overrides::apply` validates the full path shape before the first
  mutation, with a regression test pinning that a rejected spec leaves
  the tree untouched — a fix from this PR's own review round, done
  properly.
* The harness's `ini_to_config` translation (with the documented D11
  near-diagonal mapping) lets `test_lifecycle`/`test_children` run
  byte-identical test logic against both frontends — the single
  cheapest piece of oracle discipline in the whole migration.
* `rust-smoke.sh` leg 6 runs the autolaunch/SIGCHLD watch under
  valgrind and asserts the watch actually fired — the "exercised
  something real" pattern again.
* The `--examples`-doesn't-build-`--bins` cargo catch (rust-smoke leg
  1) is small but would have been a very confusing CI failure later.
* `resolve.rs` tests pin C message wording (`unknown backend
  "bogus"`, "Conflicting renderer specifications") so error-string
  parity is enforced, not aspirational.

## Cross-references

* C1 → fixed in #20 (registration after enable). C3 → #22. C4 →
  #20/#22/#23/#33/#34/#35/#36 as listed. C5 → #20. C7 → #35.
* C2 (signal shape) and C6 (color base) remain live on main — added
  to the cross-PR open-items list.
* PR16-C5's dead `SignalSource` variant went live here
  (`install_signal_sources` returns it) — annotated in the #16 review.
