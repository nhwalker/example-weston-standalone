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
cp rpm/westonite.spec /tmp/rpm/SPECS/
rpmbuild --define '_topdir /tmp/rpm' -ba /tmp/rpm/SPECS/westonite.spec
cp /tmp/rpm/RPMS/x86_64/*.rpm "$OUT"/
ls -l "$OUT"
