#!/bin/bash
# Build the Rust frontend (westonite-rs) and run the Rust-frontend e2e
# subset against it (plan §7 R2a): the re-specified CLI/config tests
# (test_cli.py, TOML + clap + `-o`) plus the lifecycle/children
# behaviors, which must pass unmodified.  The VNC lifecycle test is
# deselected until the VNC backend ports at R2c; the C frontend leg
# (e2e-test.sh) remains the oracle for everything else.
# Runs inside the containers/Containerfile.build image.
# Usage: rust-e2e-test.sh [results-dir]
set -euo pipefail

RESULTS="${1:-/tmp}"

cd /src
cargo build --locked --release -p westonite

mkdir -p "$RESULTS/failures-rust-frontend"

exec env \
	WESTONITE_BIN=/src/target/release/westonite-rs \
	WESTONITE_CONFIG_FORMAT=toml \
	WESTONITE_E2E_ARTIFACTS="$RESULTS/failures-rust-frontend" \
	python3 -m pytest tests/e2e/test_cli.py tests/e2e/test_lifecycle.py \
		tests/e2e/test_children.py \
		--deselect test_lifecycle.py::test_clean_shutdown_vnc_backend \
		-v -p no:cacheprovider \
		--junit-xml="$RESULTS/e2e-rust-frontend.xml"
