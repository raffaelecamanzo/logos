#!/usr/bin/env bash
#
# test-gate-offline-cargo.sh — prove scripts/gate.sh's offline-cargo leg turns the
# checker's exit code and summary line into the right verdict, evidence and
# printed denominator (S-805).
#
# gate.sh runs when executed, so it cannot be sourced; the functions involved are
# cut out of it by name (as test-gate-timing-assertions.sh does) and run against a
# stub scripts/check-offline-cargo.py that prints a canned summary and exits with a
# chosen code. The checker itself is tested by test-check-offline-cargo.sh; this is
# the gate's half: an unfiltered call must fail the gate, so gate.sh's own exit
# code says so, not only the evidence validator downstream.
#
# Usage: bash scripts/tests/test-gate-offline-cargo.sh
# Exit:  0 all cases behaved, 1 a case did not

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
GATE="$HERE/scripts/gate.sh"

WORK="${TMPDIR:-/tmp}"
WORK="${WORK%/}/logos-gate-offline-cargo-test.$$"
trap 'rm -rf "$WORK"' EXIT INT TERM
mkdir -p "$WORK/root/scripts" "$WORK/evid" || exit 1

extract() { # function-name -> its definition, from `name() {` to the first `^}` outside a heredoc
    awk -v n="$1" '
        $0 ~ "^"n"\\(\\) \\{" { p = 1 }
        p { print }
        p && /<<.?PY.?$/ { h = 1; next }
        p && h && /^PY$/ { h = 0; next }
        p && !h && /^}/ { exit }
    ' "$GATE"
}
FUNCS="$(for f in reset_units add_unit write_evidence record gate_offline_cargo; do extract "$f"; done)"
if ! printf '%s' "$FUNCS" | grep -q '^gate_offline_cargo()'; then
    echo "FAIL  could not extract gate_offline_cargo from gate.sh"
    exit 1
fi

# The stub checker: prints $STUB_OUT, exits $STUB_RC.
cat >"$WORK/root/scripts/check-offline-cargo.py" <<'STUB'
import os, sys
sys.stdout.write(os.environ["STUB_OUT"])
sys.exit(int(os.environ["STUB_RC"]))
STUB

PASS=0
FAIL=0

# gate <stub-rc> <stub-output> -> sets SHOWN (what the gate printed), FAILED
# (gate.sh's GATES_FAILED), and leaves the evidence in $WORK/evid.
gate() {
    rm -f "$WORK/evid/"*
    # The globals and the gate_done stub are read by the gate.sh functions
    # eval'd inside, which shellcheck cannot see.
    # shellcheck disable=SC2034,SC2329
    SHOWN="$(
        cd "$WORK/root" || exit 1
        export STUB_RC="$1" STUB_OUT="$2"
        ROOT="$WORK/root" EVID="$WORK/evid" TIER=fast TREE_ID=t HEAD_SHA=h
        WORKTREE="$WORK/root" RUNNER=r GATES_RUN="" GATES_FAILED=""
        gate_done() { return "${STUB_DONE:-1}"; }
        # STUB_NOPY=1: python3 is not on the PATH, as far as the gate can tell.
        if [ -n "${STUB_NOPY:-}" ]; then
            command() { if [ "$1 $2" = "-v python3" ]; then return 1; fi; builtin command "$@"; }
        fi
        eval "$FUNCS"
        gate_offline_cargo
        echo "GATES_FAILED=[$GATES_FAILED]"
    )"
    FAILED="$(printf '%s\n' "$SHOWN" | sed -n 's/^GATES_FAILED=\[\(.*\)\]$/\1/p')"
}

# field <json-path> -> one value from the evidence, e.g. `verdict` or `units.0.failed`
field() {
    python3 - "$WORK/evid/offline-cargo.json" "$1" <<'PY'
import json, sys
v = json.load(open(sys.argv[1]))
for k in sys.argv[2].split("."):
    v = v[int(k)] if isinstance(v, list) else v[k]
print(v)
PY
}

expect() { # expect <name> <wanted> <got>
    if [ "$2" = "$3" ]; then
        echo "  ok    $1"
        PASS=$((PASS + 1))
    else
        echo "  FAIL  $1: got '$3', wanted '$2'"
        FAIL=$((FAIL + 1))
    fi
}

echo "test-gate-offline-cargo.sh"
echo

gate 0 "offline-cargo: files=657 sites=4 metadata=2 tree=2 hits=0
"
expect "a clean scan passes the gate" "" "$FAILED"
expect "a clean scan records verdict pass" pass "$(field verdict)"
expect "the gate argv is the command it ran" "python3 scripts/check-offline-cargo.py" "$(field argv)"
expect "the unit is named sites" sites "$(field units.0.unit)"
expect "the unit argv is the command it ran" "python3 scripts/check-offline-cargo.py" "$(field units.0.argv)"
expect "the denominator is the sites found" "4 4 4 1" \
    "$(field units.0.expected) $(field units.0.started) $(field units.0.finished) $(field units.0.floor)"
expect "every site that resolves one platform is a pass" "4 0 0" \
    "$(field units.0.passed) $(field units.0.failed) $(field units.0.ignored)"
expect "the printed line carries the denominator" \
    "  offline-cargo  pass (4 offline cargo calls in 657 files scanned; 2 metadata, 2 tree, 0 unfiltered)" \
    "$(printf '%s\n' "$SHOWN" | grep '^  offline-cargo ')"

gate 1 "mcp/tests/x.rs:12: \`cargo metadata --offline\` without --filter-platform or --target resolves every target platform's crates
offline-cargo: files=657 sites=4 metadata=2 tree=2 hits=1
"
expect "a scan with hits fails the gate, so gate.sh exits 1" " offline-cargo" "$FAILED"
expect "a scan with hits records verdict fail" fail "$(field verdict)"
expect "the hits are the unit's failures and are not counted as passes" "1 3" \
    "$(field units.0.failed) $(field units.0.passed)"
expect "the hits are printed under the gate line" "    mcp/tests/x.rs:12: \`cargo metadata --offline\` without --filter-platform or --target resolves every target platform's crates" \
    "$(printf '%s\n' "$SHOWN" | grep '^    mcp/tests/x.rs')"

# Keyed by name: a reordered summary line cannot shift a count into another field.
gate 0 "offline-cargo: hits=0 tree=1 metadata=3 sites=4 files=9
"
expect "the summary is read by name, not by position" "4 4" \
    "$(field units.0.started) $(field units.0.passed)"
expect "... and the printed line splits metadata from tree the right way round" \
    "  offline-cargo  pass (4 offline cargo calls in 9 files scanned; 3 metadata, 1 tree, 0 unfiltered)" \
    "$(printf '%s\n' "$SHOWN" | grep '^  offline-cargo ')"

# At most 20 lines of hits are printed under the gate line.
many=""
for i in $(seq 1 25); do many="${many}mcp/tests/x.rs:$i: hit $i
"; done
gate 1 "${many}offline-cargo: files=9 sites=30 metadata=25 tree=5 hits=25
"
expect "25 hits print 20 lines under the gate line, and all 25 are counted" "20 25" \
    "$(printf '%s\n' "$SHOWN" | grep -c '^    mcp/tests/x.rs') $(field units.0.failed)"

# Without python3 the gate fails: it did not look, so it never passes.
STUB_NOPY=1 gate 0 "offline-cargo: files=657 sites=4 metadata=2 tree=2 hits=0
"
expect "no python3 fails the gate" " offline-cargo" "$FAILED"
expect "no python3 is recorded as exit 127 with nothing evaluated" "fail 127 no_binaries" \
    "$(field verdict) $(field exit_code) $(field truncation)"

# The same tree is not re-scanned: a cached pass is recorded and nothing runs.
STUB_DONE=0 gate 1 "offline-cargo: files=657 sites=4 metadata=2 tree=2 hits=1
"
expect "a cached pass is not re-run, so a stub that would fail cannot fail it" "" "$FAILED"
expect "... and the printed line says it was cached" "  offline-cargo  pass (cached, same tree)" \
    "$(printf '%s\n' "$SHOWN" | grep '^  offline-cargo ')"

gate 3 "offline-cargo: files=5 sites=0 metadata=0 tree=0 hits=0
"
expect "a walk that found no site fails the gate" " offline-cargo" "$FAILED"
expect "an empty walk is recorded as nothing evaluated" "fail no_binaries 0" \
    "$(field verdict) $(field truncation) $(field units.0.started)"

gate 2 "check-offline-cargo.py: not a directory: /nowhere
"
expect "a usage error fails the gate" " offline-cargo" "$FAILED"
expect "a usage error is recorded as such" "fail usage_error" "$(field verdict) $(field truncation)"

# A checker that printed no summary is not a pass over zero sites: the missing
# denominator reads as zero sites, which verify-evidence.sh rejects.
gate 0 "
"
expect "an exit 0 with no summary line records zero sites, never a populated denominator" "0 0" \
    "$(field units.0.expected) $(field units.0.started)"

echo
echo "passed $PASS, failed $FAIL"
[ "$FAIL" -eq 0 ] || exit 1
exit 0
