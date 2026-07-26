# Config migration: `westonite.ini` → `westonite.toml`

The Rust frontend (R2a, plan §5, decisions D9–D12) re-specifies the
configuration interface.  There is no converter tool and no dual-format
fallback (D11): if a legacy `westonite.ini` sits where the TOML is
expected, startup logs a one-line hint and otherwise ignores it.

An annotated example lives at the repo root:
[`westonite.toml.example`](../westonite.toml.example).

## What stays the same

- The **search order**: `$XDG_CONFIG_HOME`, then `$HOME/.config`, then
  `$XDG_CONFIG_DIRS` (default `/etc/xdg`) — each location is tried in
  turn, so setting `XDG_CONFIG_HOME` does not mask a config in
  `$HOME/.config`; `--config=PATH` and `--no-config` still work.  The
  file *name* changes (`westonite.toml` instead of `westonite.ini`),
  and one system-config path moves: libweston read the ini from a
  hard-coded `weston/` subdirectory of each `$XDG_CONFIG_DIRS` entry
  (`/etc/xdg/weston/westonite.ini`); the TOML lives in the entry
  itself (`/etc/xdg/westonite.toml`).
- **Key names**: kebab-case throughout, matching the ini's hyphenated
  keys — most entries change section syntax only, not spelling.
- **Value grammars** that are really weston's own (modelines like
  `1920x1080@60`, XKB names, transforms, gbm formats, ICC paths) are
  unchanged strings.

## What changes

| ini | TOML | Note |
|---|---|---|
| `[core]` … `key=value` | `[core]` … `key = value` | strings need quotes: `backend=headless` → `backend = "headless"` |
| `backends=drm,vnc` | `backends = ["drm", "vnc"]` | comma lists become arrays; a single comma-separated string is still accepted |
| `modules=a.so,b.so` | `modules = ["a.so", "b.so"]` | " |
| repeated `[output]` sections | `[[output]]` array-of-tables | one `[[output]]` block per output |
| `[output] mode=off` | `mode = "off"` or `off = true` | " |
| `[color_characteristics]` | `[[color-characteristics]]` | the one ini section spelled with an underscore; kebab-case like every other key |
| booleans `true`/`false` | bare `true` / `false` | unquoted |
| numbers | bare numbers | unquoted |
| `background-color=0xff002244` | `background-color = "0xff002244"` | quoted string, hex spelling kept; a bare `0xff002244` or a decimal number also works, so `-o shell.background-color=0xff002244` needs no quoting |
| unknown/typo'd keys silently ignored | **startup error** with line/column | D9: `deny_unknown_fields` |
| `WESTON_CONFIG_FILE` exported to clients | **dropped** | D12: no shipped client reads it, and no stock client parses TOML |
| `weston.ini` never read | unchanged (`westonite.toml` only) | P2 behavior kept |

## CLI

Dedicated flags keep their C spellings (`--backend`, `--socket`,
`--log`, `--width`/`--height`, `--config`, `--no-config`, trailing
autolaunch command after `--`).  New: any file key is settable with
`-o`/`--set section.key=value` (repeatable), applied to the config
tree after the file and before dedicated flags.  Override values are
read as TOML when they parse as TOML (`-o core.xwayland=true` gives a
boolean) and as plain strings otherwise, so list keys take either
spelling: `-o core.backends=drm,vnc` or
`-o 'core.backends=["drm","vnc"]'`.  Unknown flags are rejected by clap
before startup (stderr), not logged as `unhandled option:`.

`--backend` and `--backends` are one flag with two spellings, as in
C (both option-table entries write the same variable): either accepts
a comma-separated list and the last occurrence on the command line
wins.  The `[core]` keys behave the same way: `backends` is consulted
first and `backend` only as its fallback (C main.c reads them in that
order into the same variable), and either one may be a comma list —
`backend = "headless,vnc"` loads both.

`--use-gl` and `--use-pixman` remain mutually exclusive, and neither
may be combined with `--renderer` (C: `Conflicting renderer
specifications`).  `--width`, `--height` and `--scale` must be
positive; C silently treated `0` as "use the default", which hid
typos.  `[[output]] scale` follows the same rule — C passed it
straight to `weston_output_set_scale`, where `0` trips an assert.

`[[output]]` sections are validated at startup, all of them, not
lazily when a head of that name turns up: an unknown `transform` name
is fatal (C's `Invalid transform "…"` wording), as is a section with
no `name` key, and so is any key whose behaviour is not ported yet
(`clone-of`, `mirror-of`, the colour-management attributes).  C
resolves a section only when its head appears, so a section that
matches nothing is silently inert there — indistinguishable, from the
outside, from one that was honoured.  An unparseable `mode` is the
exception and stays non-fatal, matching C: it logs `Invalid mode for
output NAME. Using defaults.` and falls back to the backend default.
