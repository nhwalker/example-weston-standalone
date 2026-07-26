#!/bin/bash
# Fence-rule checks (plan §2 "Fence rules", D17) — the mechanical half.
# Runs inside the build container (or anywhere with cargo + python3 + git).
#
#  1. Dependency-graph rule: no safe crate may depend on weston-sys,
#     directly or transitively except through `weston`/`westonite-spawn`.
#     Checked on the resolved dependency graph, and every workspace
#     member must be classified safe or unsafe — a new crate that is in
#     neither list fails the check instead of silently escaping it.
#  2. forbid(unsafe_code) present in every safe crate.
#  3. Committed bindings are in sync with the installed headers (D5).
#
# The cargo-public-api snapshot check (rule 2 of §2) is NOT yet wired
# into CI — a known gap, tracked in the plan §6; do not rely on it.
set -euo pipefail

if [ -f /src/Cargo.toml ]; then cd /src; else cd "$(dirname "$0")/.."; fi

# Crate classification, per plan §2.  Grows at R2 (westonite-config,
# westonite); update here AND in the plan when it does.  Fence 1 fails
# on any workspace member missing from both lists.
SAFE_CRATES=(westonite-shell)
UNSAFE_CRATES=(weston-sys weston westonite-shell-plugin)

echo "== fence 1: dependency graph (safe crates never see weston-sys)"
META=$(mktemp)
trap 'rm -f "$META"' EXIT
cargo metadata --format-version 1 --locked > "$META"
python3 - "$META" "${#SAFE_CRATES[@]}" "${SAFE_CRATES[@]}" "${UNSAFE_CRATES[@]}" <<'EOF'
import json, sys

meta_path = sys.argv[1]
n_safe = int(sys.argv[2])
SAFE = sys.argv[3 : 3 + n_safe]
UNSAFE = sys.argv[3 + n_safe :]

meta = json.load(open(meta_path))
by_id = {p["id"]: p for p in meta["packages"]}
members = {by_id[m]["name"] for m in meta["workspace_members"]}
resolve = {n["id"]: n for n in meta["resolve"]["nodes"]}
name_to_id = {by_id[m]["name"]: m for m in meta["workspace_members"]}

bad = []

# Every workspace member must be classified.
unclassified = members - set(SAFE) - set(UNSAFE)
if unclassified:
    bad.append(
        "unclassified workspace crates (add to SAFE_CRATES or "
        f"UNSAFE_CRATES in rust-fence-check.sh AND the plan §2): "
        f"{sorted(unclassified)}"
    )

# The only crates allowed to reach weston-sys, even transitively.
FENCE = {"weston", "westonite-spawn"}

def reaches_sys(pkg_id, seen):
    """weston-sys reachable without passing through a fence crate?"""
    if pkg_id in seen:
        return False
    seen.add(pkg_id)
    for dep in resolve[pkg_id]["deps"]:
        dep_name = by_id[dep["pkg"]]["name"]
        if dep_name == "weston-sys":
            return True
        if dep_name in FENCE:
            continue  # reaching weston-sys through the fence is the design
        if dep["pkg"] in resolve and reaches_sys(dep["pkg"], seen):
            return True
    return False

for name in SAFE:
    pkg_id = name_to_id.get(name)
    if pkg_id is None:
        bad.append(f"safe crate {name} missing from workspace")
        continue
    if reaches_sys(pkg_id, set()):
        bad.append(f"safe crate {name} reaches weston-sys outside the fence")

if bad:
    print("\n".join(bad)); sys.exit(1)
print(f"ok ({len(SAFE)} safe crates, {len(members)} members classified)")
EOF

echo "== fence 2: forbid(unsafe_code) in safe crates"
for c in "${SAFE_CRATES[@]:-}"; do
	[ -z "$c" ] && continue
	# Anchored to line start so a doc-comment mention cannot satisfy it.
	grep -q '^#!\[forbid(unsafe_code)\]' "crates/$c/src/lib.rs" \
		|| { echo "FAIL: crates/$c missing forbid(unsafe_code)"; exit 1; }
	grep -q '^unsafe_code *= *"forbid"' "crates/$c/Cargo.toml" \
		|| { echo "FAIL: crates/$c Cargo.toml missing unsafe_code = \"forbid\" lint"; exit 1; }
done
echo "ok"

echo "== fence 3: committed bindings match the installed headers"
if command -v bindgen >/dev/null 2>&1 || [ -n "${FENCE_CHECK_BINDINGS:-}" ]; then
	# Regenerate to a scratch copy: the committed file (and its mtime,
	# which cargo's rerun-if-changed watches) must not be touched.
	REGEN=$(mktemp)
	trap 'rm -f "$META" "$REGEN"' EXIT
	REGEN_BINDINGS_OUT="$REGEN" ./scripts/regen-bindings.sh >/dev/null
	if ! diff -u crates/weston-sys/src/bindings.rs "$REGEN" >/dev/null; then
		echo "FAIL: bindings.rs is stale — run scripts/regen-bindings.sh and commit"
		exit 1
	fi
	echo "ok"
else
	# Local-dev convenience only.  CI sets FENCE_CHECK_BINDINGS=1 (and
	# the build container ships a pinned bindgen-cli), so the check can
	# never be skipped silently there.
	echo "skipped (bindgen CLI not installed; set FENCE_CHECK_BINDINGS=1 to force)"
fi

echo "ALL FENCE CHECKS PASSED"
