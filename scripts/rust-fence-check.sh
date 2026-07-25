#!/bin/bash
# Fence-rule checks (plan §2 "Fence rules", D17) — the mechanical half.
# Runs inside the build container (or anywhere with cargo + python3 + git).
#
#  1. Dependency-graph rule: no safe crate may depend on weston-sys
#     (directly or transitively except through `weston`/`westonite-spawn`).
#  2. forbid(unsafe_code) present in every safe crate.
#  3. Committed bindings are in sync with the installed headers (D5).
#
# The cargo-public-api snapshot check (rule 2 of §2) runs in the nightly
# CI job — see .github/workflows/ci.yml.
set -euo pipefail

cd /src 2>/dev/null || cd "$(dirname "$0")/.."

# Safe crates, per plan §2.  Grows at R1/R2 (westonite-config,
# westonite-shell, westonite); update here AND in the plan when it does.
SAFE_CRATES=()
UNSAFE_CRATES=(weston-sys weston)

echo "== fence 1: dependency graph (safe crates never see weston-sys)"
cargo metadata --format-version 1 --no-deps > /tmp/meta.json
python3 - "${SAFE_CRATES[@]:-}" <<'EOF'
import json, sys

SAFE = [a for a in sys.argv[1:] if a]
meta = json.load(open("/tmp/meta.json"))
by_name = {p["name"]: p for p in meta["packages"]}
bad = []
for name in SAFE:
    pkg = by_name.get(name)
    if pkg is None:
        bad.append(f"safe crate {name} missing from workspace")
        continue
    deps = {d["name"] for d in pkg["dependencies"]}
    if "weston-sys" in deps:
        bad.append(f"safe crate {name} depends on weston-sys")
if bad:
    print("\n".join(bad)); sys.exit(1)
print(f"ok ({len(SAFE)} safe crates checked)")
EOF

echo "== fence 2: forbid(unsafe_code) in safe crates"
for c in "${SAFE_CRATES[@]:-}"; do
	[ -z "$c" ] && continue
	grep -q '#!\[forbid(unsafe_code)\]' "crates/$c/src/lib.rs" \
		|| { echo "FAIL: crates/$c missing forbid(unsafe_code)"; exit 1; }
done
echo "ok"

echo "== fence 3: committed bindings match the installed headers"
if command -v bindgen >/dev/null 2>&1 || [ -n "${FENCE_CHECK_BINDINGS:-}" ]; then
	cp crates/weston-sys/src/bindings.rs /tmp/bindings.committed.rs
	./scripts/regen-bindings.sh >/dev/null
	if ! diff -q /tmp/bindings.committed.rs crates/weston-sys/src/bindings.rs >/dev/null; then
		mv /tmp/bindings.committed.rs crates/weston-sys/src/bindings.rs
		echo "FAIL: bindings.rs is stale — run scripts/regen-bindings.sh and commit"
		exit 1
	fi
	echo "ok"
else
	echo "skipped (bindgen CLI not installed; set FENCE_CHECK_BINDINGS=1 to force)"
fi

echo "ALL FENCE CHECKS PASSED"
