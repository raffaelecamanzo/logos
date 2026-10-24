#!/usr/bin/env bash
#
# test-install-dist.sh — prove scripts/install-dist.sh runs the cargo-dist
# installer only when its sha256 is the pinned one (HF-4).
#
# Nothing here reaches the network. $CURL points at a stub that records its
# arguments and copies $STUB_BODY to the `-o` file (or fails, as curl does); the
# "installer" is a fixture that records that it ran. The pin the script holds
# is the real cargo-dist 0.32.0 installer's, which the fixture cannot match, so
# the matching cases run a copy of the script whose pin is the fixture's own digest
# (the script reads its pin from no environment variable, so none can replace it).
#
# Usage: bash scripts/tests/test-install-dist.sh
# Exit:  0 all cases behaved, 1 a case did not
#
# bash 3.2 compatible.

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRIPT="$HERE/scripts/install-dist.sh"
[ -f "$SCRIPT" ] || { echo "cannot find scripts/install-dist.sh (looked in $HERE)" >&2; exit 1; }

WORK="${TMPDIR:-/tmp}/logos-install-dist-test.$$"
trap 'rm -rf "$WORK"' EXIT INT TERM
mkdir -p "$WORK/bin" "$WORK/tmp" || exit 1
WORK="$(cd "$WORK" && pwd -P)"

cat >"$WORK/bin/curl" <<'STUB'
#!/usr/bin/env bash
printf '%s\n' "$@" >"$STUB_LOG"
if [ -n "${STUB_FAIL:-}" ]; then exit 22; fi
out=""
while [ $# -gt 0 ]; do
    if [ "$1" = "-o" ]; then out="$2"; shift; fi
    shift
done
cat "$STUB_BODY" >"$out"
STUB
chmod +x "$WORK/bin/curl"
export CURL="$WORK/bin/curl" STUB_LOG="$WORK/curl.log" STUB_BODY="$WORK/installer.sh" RUNNER_TEMP="$WORK/tmp"
export MARK="$WORK/ran"

printf '#!/bin/sh\necho ran >"$MARK"\n' >"$STUB_BODY"
GOOD="$(shasum -a 256 "$STUB_BODY" | cut -d' ' -f1)"

PASS=0
FAIL=0
OUT=""
ok() { echo "  ok    $1"; PASS=$((PASS + 1)); }
bad() { echo "  FAIL  $1"; printf '%s\n' "$OUT" | sed 's/^/        /'; FAIL=$((FAIL + 1)); }

# run [PIN] — the shipped script, or with PIN a copy of it that pins PIN instead.
run() {
    local script="$SCRIPT"
    if [ $# -gt 0 ]; then
        script="$WORK/install-dist.pinned.sh"
        sed "s/^DIST_INSTALLER_SHA256=\"[0-9a-f]*\"\$/DIST_INSTALLER_SHA256=\"$1\"/" "$SCRIPT" >"$script"
        grep -q "\"$1\"" "$script" || { OUT="the pin was not patched"; bad "test setup"; return; }
    fi
    rm -f "$MARK" "$STUB_LOG"; OUT="$(bash "$script" 2>&1)"; RC=$?
}

echo "test-install-dist.sh"
echo

# The pin the script carries is the 64-hex digest of the v0.32.0 installer.
pin="$(sed -n 's/^DIST_INSTALLER_SHA256="\([0-9a-f]*\)"$/\1/p' "$SCRIPT")"
if [ "${#pin}" -eq 64 ]; then ok "the script pins a 64-hex sha256"; else OUT="pin read: '$pin'"; bad "no 64-hex pin in the script"; fi

# The version the script downloads is the one dist-workspace.toml asks CI for.
want="$(sed -n 's/^cargo-dist-version = "\([0-9.]*\)"$/\1/p' "$HERE/dist-workspace.toml")"
have="$(sed -n 's/^DIST_VERSION="\([0-9.]*\)"$/\1/p' "$SCRIPT")"
if [ -n "$want" ] && [ "$want" = "$have" ]; then ok "DIST_VERSION is dist-workspace.toml's cargo-dist-version ($want)"; else OUT="script '$have', dist-workspace.toml '$want'"; bad "DIST_VERSION and cargo-dist-version differ"; fi

# An environment variable of the old name cannot replace the pin.
DIST_INSTALLER_SHA256="$(shasum -a 256 "$STUB_BODY" | cut -d' ' -f1)" run
if [ "$RC" -eq 1 ] && [ ! -f "$MARK" ]; then ok "an environment variable cannot replace the pin"; else bad "the pin was replaced from the environment (exit $RC)"; fi

run "$GOOD"
if [ "$RC" -eq 0 ] && [ -f "$MARK" ]; then ok "a download that matches the pin is run"; else bad "a matching installer was not run (exit $RC)"; fi

if grep -qx -- '--tlsv1.2' "$STUB_LOG" && grep -qx -- "--proto" "$STUB_LOG" && grep -qx -- '=https' "$STUB_LOG" &&
    grep -qx 'https://github.com/axodotdev/cargo-dist/releases/download/v0.32.0/cargo-dist-installer.sh' "$STUB_LOG"; then
    ok "the download is https-only, TLS 1.2+, of the v0.32.0 installer"
else OUT="$(cat "$STUB_LOG")"; bad "the download is not the pinned https URL"; fi

run "$GOOD"
if ls "$RUNNER_TEMP" | grep -q .; then OUT="$(ls "$RUNNER_TEMP")"; bad "the downloaded installer was left behind"; else ok "the downloaded installer is removed after the run"; fi

# A near miss: one digit off must refuse and run nothing.
# A near miss: one character off, at the start, the middle and the end, must refuse
# and run nothing, and the refused download must not be left behind.
flip() { case "${GOOD:$1:1}" in 0) c=1 ;; *) c=0 ;; esac; printf '%s%s%s' "${GOOD:0:$1}" "$c" "${GOOD:$(($1 + 1))}"; }
for at in 0 31 63; do
    run "$(flip $at)"
    if [ "$RC" -eq 1 ] && [ ! -f "$MARK" ] && printf '%s\n' "$OUT" | grep -q 'Nothing was run'; then ok "a digest differing at character $at is refused and nothing runs"; else bad "a mismatched installer (character $at) was not refused (exit $RC)"; fi
    if ls "$RUNNER_TEMP" | grep -q .; then OUT="$(ls "$RUNNER_TEMP")"; bad "a refused installer was left behind (character $at)"; fi
done

# The real pin does not match the fixture: the default path refuses a swapped installer.
unset DIST_INSTALLER_SHA256
run
if [ "$RC" -eq 1 ] && [ ! -f "$MARK" ]; then ok "an installer other than the pinned one is refused by the shipped pin"; else bad "the shipped pin accepted the fixture (exit $RC)"; fi

# An empty body (a truncated download that curl called a success) is a mismatch.
: >"$STUB_BODY"
run
if [ "$RC" -eq 1 ] && [ ! -f "$MARK" ]; then ok "an empty download is refused"; else bad "an empty download was not refused (exit $RC)"; fi
printf '#!/bin/sh\necho ran >"$MARK"\n' >"$STUB_BODY"

STUB_FAIL=1 run "$GOOD"
if [ "$RC" -eq 1 ] && [ ! -f "$MARK" ] && printf '%s\n' "$OUT" | grep -q 'cannot download'; then ok "a failed download stops before anything runs"; else bad "a failed download did not stop the script (exit $RC)"; fi

# A host with `shasum` alone: cargo-dist's installer verifies each archive with
# `sha256sum` and skips the check without one, so the script gives it one. The
# fixture installer computes its own digest the way the real one does.
# path_with TOOL... — a PATH directory holding only the named tools.
path_with() {
    local dir="$WORK/path.$1" tool; shift
    rm -rf "$dir"; mkdir -p "$dir"
    for tool in "$@"; do ln -s "$(command -v "$tool")" "$dir/$tool" || return 1; done
    echo "$dir"
}
BASH_BIN="$(command -v bash)"
printf '#!/bin/sh\ncommand -v sha256sum >/dev/null || exit 3\nsha256sum -b "$0" | awk '"'"'{printf $1}'"'"' >"$MARK"\n' >"$STUB_BODY"
OWN="$(shasum -a 256 "$STUB_BODY" | cut -d' ' -f1)"
sed "s/^DIST_INSTALLER_SHA256=\"[0-9a-f]*\"\$/DIST_INSTALLER_SHA256=\"$OWN\"/" "$SCRIPT" >"$WORK/install-dist.own.sh"
P="$(path_with shasum-only bash sh env cut rm cat awk mkdir chmod shasum)"
rm -f "$MARK"; OUT="$(PATH="$P" "$BASH_BIN" "$WORK/install-dist.own.sh" 2>&1)"; RC=$?
if [ "$RC" -eq 0 ] && [ "$(cat "$MARK" 2>/dev/null)" = "$OWN" ]; then ok "with shasum alone, the installer still has a sha256sum to verify archives with"; else bad "with shasum alone, the installer had no working sha256sum (exit $RC, digest '$(cat "$MARK" 2>/dev/null)')"; fi
if ls "$RUNNER_TEMP" | grep -q .; then OUT="$(ls "$RUNNER_TEMP")"; bad "the sha256sum shim or the installer was left behind"; else ok "the sha256sum shim is removed after the run"; fi

P="$(path_with none bash sh env cut rm cat awk mkdir chmod)"
rm -f "$MARK" "$STUB_LOG"; OUT="$(PATH="$P" "$BASH_BIN" "$WORK/install-dist.own.sh" 2>&1)"; RC=$?
if [ "$RC" -eq 2 ] && [ ! -f "$MARK" ] && [ ! -f "$STUB_LOG" ]; then ok "with no checksum tool it exits 2 before downloading or running anything"; else bad "with no checksum tool: exit $RC, ran: $([ -f "$MARK" ] && echo yes || echo no)"; fi
printf '#!/bin/sh\necho ran >"$MARK"\n' >"$STUB_BODY"

echo
echo "passed $PASS, failed $FAIL"
[ "$FAIL" -eq 0 ] || exit 1
exit 0
