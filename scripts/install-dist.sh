#!/usr/bin/env bash
#
# install-dist.sh — install the pinned cargo-dist, verified (HF-4)
#
#   bash scripts/install-dist.sh
#
# The "Install dist" step of .github/workflows/release.yml. cargo-dist's own
# snippet pipes its installer into `sh`, which runs whatever the URL serves that
# day. This downloads the installer to a file, refuses it unless its sha256 is the
# one pinned below, and only then runs it. The installer carries a sha256 for each
# platform archive it fetches, so pinning the script pins the chain behind it. The
# installer checks those with `sha256sum` and skips the check, with only a message,
# when there is none; so on a host with `shasum` alone it runs with a `sha256sum`
# that calls `shasum -a 256`, and the archive is never installed unverified.
#
# To move to another dist version, download the new installer, read it, and change
# DIST_VERSION and DIST_INSTALLER_SHA256 together; `cargo-dist-version` in
# dist-workspace.toml names the same version.
#
# Needs: curl, shasum or sha256sum, sh. $CURL names the curl binary to run
# (scripts/tests/test-install-dist.sh points it at a stub); the pin is a literal
# here and nothing in the environment can replace it.
# Exit: 0 installed; 1 the download failed or the checksum differs (nothing was
# run); 2 no checksum tool; otherwise the installer's own status.

set -euo pipefail

DIST_VERSION="0.32.0"
DIST_INSTALLER_SHA256="b657cf8c04a8b7bc28f39d220f7e6dd11bbd2bdb072c552262bd9ccf597261b5"
CURL="${CURL:-curl}"
URL="https://github.com/axodotdev/cargo-dist/releases/download/v${DIST_VERSION}/cargo-dist-installer.sh"

DIR="${RUNNER_TEMP:-${TMPDIR:-/tmp}}"
INSTALLER="$DIR/cargo-dist-installer.$$.sh"
SHIM="$DIR/install-dist-bin.$$"
trap 'rm -rf "$INSTALLER" "$SHIM"' EXIT

if command -v sha256sum >/dev/null 2>&1; then
    sha256_of() { sha256sum "$1" | cut -d' ' -f1; }
elif command -v shasum >/dev/null 2>&1; then
    sha256_of() { shasum -a 256 "$1" | cut -d' ' -f1; }
    mkdir "$SHIM"
    printf '#!/bin/sh\nexec shasum -a 256 "$@"\n' >"$SHIM/sha256sum"
    chmod +x "$SHIM/sha256sum"
    PATH="$SHIM:$PATH"
else
    echo "install-dist.sh: neither sha256sum nor shasum is installed" >&2
    exit 2
fi

"$CURL" --proto '=https' --tlsv1.2 -LsSf -o "$INSTALLER" "$URL" ||
    { echo "::error::install-dist: cannot download $URL" >&2; exit 1; }

got="$(sha256_of "$INSTALLER")"
if [ "$got" != "$DIST_INSTALLER_SHA256" ]; then
    echo "::error::install-dist: $URL has sha256 $got, not the pinned $DIST_INSTALLER_SHA256. Nothing was run." >&2
    exit 1
fi

sh "$INSTALLER"
