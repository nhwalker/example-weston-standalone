#!/bin/bash
# Build the westonite RPM from the /src checkout.
# Runs inside the containers/Containerfile.build image.
# Usage: rpm-build.sh [output-dir]   (default: /src/build-rpms)
set -euo pipefail

OUT="${1:-/src/build-rpms}"
VERSION=$(sed -n 's/^Version: *//p' /src/rpm/westonite.spec)

git config --global --add safe.directory /src
mkdir -p /tmp/rpm/{SOURCES,SPECS} "$OUT"
cd /src
git archive --prefix="westonite-$VERSION/" \
	-o "/tmp/rpm/SOURCES/westonite-$VERSION.tar.gz" HEAD
# Vendor the cargo registry deps (Source1, plan §6) so the spec's cargo
# build runs --offline: rpmbuild itself never touches the network.
rm -rf /tmp/rpm/vendor-work
mkdir -p /tmp/rpm/vendor-work
cargo vendor --locked /tmp/rpm/vendor-work/vendor >/dev/null
tar -C /tmp/rpm/vendor-work -czf \
	"/tmp/rpm/SOURCES/westonite-vendor-$VERSION.tar.gz" vendor
cp rpm/westonite.spec /tmp/rpm/SPECS/
rpmbuild --define '_topdir /tmp/rpm' -ba /tmp/rpm/SPECS/westonite.spec
cp /tmp/rpm/RPMS/x86_64/*.rpm "$OUT"/
ls -l "$OUT"
