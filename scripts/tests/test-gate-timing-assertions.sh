#!/usr/bin/env bash
#
# test-gate-timing-assertions.sh — prove scripts/gate.sh's timing-assertions leg
# turns the checker's exit code and summary line into the right verdict, evidence
# and printed denominator (S-804).
#
# gate.sh runs when executed, so it cannot be sourced; the functions involved are
# cut out of it by name (as test-gate-public-terms.sh does) and run against a stub
# scripts/check-timing-assertions.py that prints a canned summary and exits with a
# chosen code. The checker itself is tested by test-check-timing-assertions.sh;
# this is the gate's half: a bypass must fail the gate, so gate.sh's own exit code
# says so, not only the evidence validator downstream.
#
# Usage: bash scripts/tests/test-gate-timing-assertions.sh
# Exit:  0 all cases behaved, 1 a case did not

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
GATE="$HERE/scripts/gate.sh"

WORK="${TMPDIR:-/tmp}"
WORK="${WORK%/}/logos-gate-timing-assertions-test.$$"
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
FUNCS="$(for f in reset_units add_unit write_evidence record gate_timing_assertions; do extract "$f"; done)"
if ! printf '%s' "$FUNCS" | grep -q '^gate_timing_assertions()'; then
    echo "FAIL  could not extract gate_timing_assertions from gate.sh"
    exit 1
fi

# The stub checker: prints $STUB_OUT, exits $STUB_RC.
cat >"$WORK/root/scripts/check-timing-assertions.py" <<'EOF'
import os, sys
sys.stdout.write(os.environ["STUB_OUT"])
sys.exit(int(os.environ["STUB_RC"]))
EOF

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
        gate_done() { return 1; }
        eval "$FUNCS"
        gate_timing_assertions
        echo "GATES_FAILED=[$GATES_FAILED]"
    )"
    FAILED="$(printf '%s\n' "$SHOWN" | sed -n 's/^GATES_FAILED=\[\(.*\)\]$/\1/p')"
}

# field <json-path> -> one value from the evidence, e.g. `verdict` or `units.0.failed`
field() {
    python3 - "$WORK/evid/timing-assertions.json" "$1" <<'PY'
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

echo "test-gate-timing-assertions.sh"
echo

gate 0 "timing-assertions: files=620 sites=71 scaled=70 lower=1 hits=0
"
expect "a clean scan passes the gate" "" "$FAILED"
expect "a clean scan records verdict pass" pass "$(field verdict)"
expect "the gate argv is the command it ran" "python3 scripts/check-timing-assertions.py" "$(field argv)"
expect "the unit argv is the command it ran" "python3 scripts/check-timing-assertions.py" "$(field units.0.argv)"
expect "the denominator is the sites found" "71 71 71 1" \
    "$(field units.0.expected) $(field units.0.started) $(field units.0.finished) $(field units.0.floor)"
expect "scaled sites and minimums are the passes, minimums also the ignored" "71 0 1" \
    "$(field units.0.passed) $(field units.0.failed) $(field units.0.ignored)"
expect "the printed line carries both denominators" \
    "  timing-assertions pass (71 timing assertions in 620 files scanned; 70 scaled, 1 minimums, 0 unscaled)" \
    "$(printf '%s\n' "$SHOWN" | grep '^  timing-assertions ')"

gate 1 "cli/tests/a.rs:10: elapsed-time upper bound not scaled through logos_core::perf::scaled: t < d
core/b.rs:99: elapsed-time upper bound not scaled through logos_core::perf::scaled: e < 5
timing-assertions: files=620 sites=71 scaled=68 lower=1 hits=2
"
expect "a scan with hits fails the gate, so gate.sh exits 1" " timing-assertions" "$FAILED"
expect "a scan with hits records verdict fail" fail "$(field verdict)"
expect "the hits are the unit's failures and are not counted as passes" "2 69" \
    "$(field units.0.failed) $(field units.0.passed)"
expect "the hits are printed under the gate line" "    cli/tests/a.rs:10: elapsed-time upper bound not scaled through logos_core::perf::scaled: t < d" \
    "$(printf '%s\n' "$SHOWN" | grep '^    cli/tests/a.rs')"

# Keyed by name: a reordered summary line cannot shift a count into another field.
gate 0 "timing-assertions: hits=0 lower=2 scaled=30 sites=32 files=9
"
expect "the summary is read by name, not by position" "32 2" \
    "$(field units.0.started) $(field units.0.ignored)"

gate 3 "timing-assertions: files=5 sites=0 scaled=0 lower=0 hits=0
"
expect "a walk that found no site fails the gate" " timing-assertions" "$FAILED"
expect "an empty walk is recorded as nothing evaluated" "fail no_binaries 0" \
    "$(field verdict) $(field truncation) $(field units.0.started)"

gate 2 "check-timing-assertions.py: not a directory: /nowhere
"
expect "a usage error fails the gate" " timing-assertions" "$FAILED"
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
