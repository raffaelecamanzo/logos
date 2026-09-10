#!/usr/bin/env bash
#
# test-verify-evidence.sh — prove that verify-evidence.sh rejects each way a
# gate has historically reported success while failing.
#
# This test exists because the failure it guards against is silence: a validator
# that accepts everything looks exactly like a validator that works. Every case
# below crafts evidence that a naive reader would call green, and asserts the
# validator says no. The last case crafts genuinely valid evidence and asserts it
# says yes — without it, a validator that rejects unconditionally would pass.
#
# Runs in a throwaway git repository so it can never touch real evidence.
#
# Usage: bash scripts/tests/test-verify-evidence.sh
# Exit:  0 all cases behaved, 1 a case did not

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
VERIFY="$HERE/scripts/verify-evidence.sh"

if [ ! -f "$VERIFY" ]; then
    echo "cannot find scripts/verify-evidence.sh (looked in $HERE)" >&2
    exit 1
fi

WORK="${TMPDIR:-/tmp}/logos-verify-evidence-test.$$"
trap 'rm -rf "$WORK"' EXIT INT TERM
mkdir -p "$WORK/repo/scripts" || exit 1

cd "$WORK/repo" || exit 1
git init -q . >/dev/null 2>&1
git config user.email t@example.com
git config user.name test
cp "$VERIFY" scripts/verify-evidence.sh
echo "seed" >file.txt
git add -A >/dev/null 2>&1
git commit -qm seed >/dev/null 2>&1

EVID="$(git rev-parse --path-format=absolute --git-path gate-evidence)"
TREE="$(git rev-parse HEAD^{tree})"

PASS=0
FAIL=0

# Write one evidence file. Args: gate tier verdict tree_id units-json
write_gate() {
    mkdir -p "$EVID"
    cat >"$EVID/$1.json" <<EOF
{
  "schema": 1,
  "gate": "$1",
  "tier": "$2",
  "tree_id": "$4",
  "head": "deadbeef",
  "worktree": "$WORK/repo",
  "argv": "synthetic",
  "exit_code": 0,
  "verdict": "$3",
  "truncation": "none",
  "units": $5,
  "log": "/dev/null",
  "runner": "synthetic"
}
EOF
}

# A unit whose counters are internally consistent and complete.
good_unit() {
    printf '[{"unit":"p","expected":10,"floor":9,"started":10,"finished":10,'
    printf '"passed":100,"failed":0,"ignored":0,"exit_code":0,'
    printf '"verdict":"pass","truncation":"none"}]'
}

# Lay down a full, valid evidence set, then let each case corrupt one thing.
seed_all_valid() {
    rm -rf "$EVID"
    local g
    for g in clippy test test-agents deny arch ui-typecheck ui-test ui-build; do
        case "$g" in
            test | test-agents) write_gate "$g" full pass "$TREE" "$(good_unit)" ;;
            *) write_gate "$g" full pass "$TREE" "[]" ;;
        esac
    done
}

# check <name> <expected-exit> <grep-pattern-that-must-appear>
check() {
    local name="$1" want="$2" pattern="$3" out rc
    out="$(bash scripts/verify-evidence.sh --strict 2>&1)"
    rc=$?
    if [ "$rc" -ne "$want" ]; then
        echo "  FAIL  $name: exit $rc, wanted $want"
        echo "$out" | sed 's/^/        /'
        FAIL=$((FAIL + 1))
        return
    fi
    if [ -n "$pattern" ] && ! printf '%s\n' "$out" | grep -q "$pattern"; then
        echo "  FAIL  $name: exit $rc correct, but no message matching '$pattern'"
        echo "$out" | sed 's/^/        /'
        FAIL=$((FAIL + 1))
        return
    fi
    echo "  ok    $name"
    PASS=$((PASS + 1))
}

echo "test-verify-evidence.sh"
echo "  repo $WORK/repo"
echo "  tree $TREE"
echo

# --- the four historical failure modes, each crafted to look green -------------

seed_all_valid
rm -f "$EVID/deny.json"
check "a gate that never ran is not a pass" 1 "deny: no evidence file"

seed_all_valid
write_gate deny full skip "$TREE" "[]"
check "a skipped gate (tool absent) is not a pass" 1 "deny: skipped"

# Cargo's target-level fail-fast never LAUNCHES the later binaries, so they print
# neither `Running` nor a summary: started == finished, both below expected. That
# is what distinguishes truncation from an OOM kill, where a binary announced
# itself and then died (started > finished).
seed_all_valid
write_gate test full pass "$TREE" "$(
    printf '[{"unit":"p","expected":10,"floor":9,"started":6,"finished":6,'
    printf '"passed":60,"failed":0,"ignored":0,"exit_code":0,'
    printf '"verdict":"pass","truncation":"none"}]'
)"
check "fail-fast truncation (6 of 10 binaries) rejected" 1 "6 of 10 binaries ran"

seed_all_valid
write_gate test full pass "$TREE" "$(
    printf '[{"unit":"p","expected":10,"floor":9,"started":10,"finished":9,'
    printf '"passed":90,"failed":0,"ignored":0,"exit_code":0,'
    printf '"verdict":"pass","truncation":"none"}]'
)"
check "killed mid-run (started > finished) rejected" 1 "was killed"

seed_all_valid
write_gate test full pass "$TREE" "$(
    printf '[{"unit":"p","expected":10,"floor":9,"started":0,"finished":0,'
    printf '"passed":0,"failed":0,"ignored":0,"exit_code":0,'
    printf '"verdict":"pass","truncation":"none"}]'
)"
check "zero binaries executed rejected (the 0-of-0 trap)" 1 "no test binary executed"

# --- fabricated fields ---------------------------------------------------------

seed_all_valid
write_gate test full pass "$TREE" "$(
    printf '[{"unit":"p","expected":10,"floor":9,"started":10,"finished":10,'
    printf '"passed":90,"failed":4,"ignored":0,"exit_code":0,'
    printf '"verdict":"pass","truncation":"none"}]'
)"
check "a 'pass' contradicting its own counters rejected" 1 "contradicts its own counters"

seed_all_valid
write_gate clippy full pass "$TREE" "[]"
python3 - "$EVID/clippy.json" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
d["exit_code"] = 101          # the `| tail` mask: verdict says pass, status says 101
json.dump(d, open(sys.argv[1], "w"))
PY
check "verdict 'pass' with a non-zero exit code rejected" 1 'exit_code 101'

# --- staleness and the fast/full split ----------------------------------------

seed_all_valid
write_gate test fast pass "$TREE" "$(good_unit)"
check "fast-tier evidence never satisfies handoff" 1 'not "full"'

seed_all_valid
write_gate test full pass "0000000000000000000000000000000000000000" "$(good_unit)"
check "gates disagreeing about the tree rejected" 1 "disagree about the tree"

seed_all_valid
echo "an edit made after the gates ran" >>file.txt
check "evidence older than the working tree rejected" 1 "stale evidence"
git checkout -- file.txt 2>/dev/null

# --- and the control: valid evidence must be ACCEPTED -------------------------
#
# Without this case, a validator that rejected everything would score 10/10.

seed_all_valid
check "genuinely valid full-tier evidence accepted" 0 "EVIDENCE VERIFIED"

# --- the marker is only written on acceptance ---------------------------------

seed_all_valid
mkdir -p "$WORK/pending"
if bash scripts/verify-evidence.sh --strict \
    --write-marker "$WORK/pending/S-000-review-done" >/dev/null 2>&1 &&
    grep -q "EVIDENCE-VERIFIED tree=$TREE" "$WORK/pending/S-000-review-done"; then
    echo "  ok    marker written with the verified tree id embedded"
    PASS=$((PASS + 1))
else
    echo "  FAIL  marker not written, or missing the tree id"
    FAIL=$((FAIL + 1))
fi

rm -f "$WORK/pending/S-000-review-done"
seed_all_valid
rm -f "$EVID/deny.json"
bash scripts/verify-evidence.sh --strict \
    --write-marker "$WORK/pending/S-000-review-done" >/dev/null 2>&1
if [ -e "$WORK/pending/S-000-review-done" ]; then
    echo "  FAIL  marker was written despite rejected evidence"
    FAIL=$((FAIL + 1))
else
    echo "  ok    no marker written when the evidence is rejected"
    PASS=$((PASS + 1))
fi

echo
echo "passed $PASS, failed $FAIL"
[ "$FAIL" -eq 0 ] || exit 1
exit 0
