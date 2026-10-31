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
# Env:   VERIFY_SH  path of the verifier under test (default: scripts/verify-evidence.sh)
# Exit:  0 all cases behaved, 1 a case did not

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
VERIFY="${VERIFY_SH:-$HERE/scripts/verify-evidence.sh}"

if [ ! -f "$VERIFY" ]; then
    echo "cannot find scripts/verify-evidence.sh (looked in $HERE)" >&2
    exit 1
fi

WORK="${TMPDIR:-/tmp}/logos-verify-evidence-test.$$"
trap 'rm -rf "$WORK"' EXIT INT TERM
mkdir -p "$WORK/repo/scripts/tests" || exit 1

cd "$WORK/repo" || exit 1
git init -q . >/dev/null 2>&1
git config user.email t@example.com
git config user.name test
cp "$VERIFY" scripts/verify-evidence.sh
# Two suites the tree "holds", for the script-suites gate's per-suite units.
: >scripts/tests/test-alpha.sh
: >scripts/tests/test-beta.sh
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

# The public-terms gate's one unit (gate.sh's gate_public_terms): the text files
# its walk read, and the term hits outside the allowlist as its failures.
# Args: files-read hits
files_unit() {
    printf '[{"unit":"files","expected":%d,"floor":1,"started":%d,"finished":%d,' "$1" "$1" "$1"
    printf '"passed":%d,"failed":%d,"ignored":5,"exit_code":0,' "$1" "$2"
    printf '"verdict":"pass","truncation":"none"}]'
}

# The timing-assertions gate's one unit (gate.sh's gate_timing_assertions): the
# elapsed-time assertions its walk found, and the upper bounds that bypass
# logos_core::perf as its failures. Args: sites-found hits
sites_unit() {
    printf '[{"unit":"sites","expected":%d,"floor":1,"started":%d,"finished":%d,' "$1" "$1" "$1"
    printf '"passed":%d,"failed":%d,"ignored":1,"exit_code":0,' "$(($1 - $2))" "$2"
    printf '"verdict":"pass","truncation":"none"}]'
}

# The offline-cargo gate's one unit (gate.sh's gate_offline_cargo): the offline cargo
# calls its walk found, and the unfiltered ones as its failures.
# Args: sites-found hits
cargo_unit() {
    printf '[{"unit":"sites","expected":%d,"floor":1,"started":%d,"finished":%d,' "$1" "$1" "$1"
    printf '"passed":%d,"failed":%d,"ignored":0,"exit_code":0,' "$(($1 - $2))" "$2"
    printf '"verdict":"pass","truncation":"none"}]'
}

# The agents-marker gate's one unit (gate.sh's gate_agents_marker): the gated tests
# scripts/agents-marker.sh names, against those that printed a result line.
# Args: expected started finished failed
gated_unit() {
    printf '[{"unit":"gated","expected":%d,"floor":1,"started":%d,"finished":%d,' "$1" "$2" "$3"
    printf '"passed":88,"failed":%d,"ignored":0,"exit_code":0,' "$4"
    printf '"verdict":"pass","truncation":"none"}]'
}

# The script-suites gate's units (gate.sh's gate_script_suites): one per suite, a
# run to an exit being 1 of 1; passed/failed are the checks the suite counted.
# Args: name passed failed exit-code
suite_unit() {
    printf '{"unit":"%s","expected":1,"floor":1,"started":1,"finished":1,' "$1"
    printf '"passed":%d,"failed":%d,"ignored":0,"exit_code":%d,' "$2" "$3" "$4"
    if [ "$3" -eq 0 ] && [ "$4" -eq 0 ]; then printf '"verdict":"pass","truncation":"none"}'; else printf '"verdict":"fail","truncation":"none"}'; fi
}
# Both suites green. Args: (none)
suites_valid() {
    printf '[%s,%s]' "$(suite_unit test-alpha 12 0 0)" "$(suite_unit test-beta 7 0 0)"
}

# Lay down a valid evidence set at a tier (default full), then let each case
# corrupt one thing.
seed_all_valid() {
    local tier="${1:-full}"
    rm -rf "$EVID"
    local g
    for g in clippy test agents-marker deny arch public-terms timing-assertions offline-cargo script-suites ui-typecheck ui-test ui-build ui-e2e; do
        case "$g" in
            script-suites) write_gate "$g" "$tier" pass "$TREE" "$(suites_valid)" ;;
            agents-marker) write_gate "$g" "$tier" pass "$TREE" "$(gated_unit 1 1 1 0)" ;;
            test) write_gate "$g" "$tier" pass "$TREE" "$(good_unit)" ;;
            public-terms) write_gate "$g" "$tier" pass "$TREE" "$(files_unit 1044 0)" ;;
            timing-assertions) write_gate "$g" "$tier" pass "$TREE" "$(sites_unit 71 0)" ;;
            offline-cargo) write_gate "$g" "$tier" pass "$TREE" "$(cargo_unit 4 0)" ;;
            *) write_gate "$g" "$tier" pass "$TREE" "[]" ;;
        esac
    done
}

# The tier check() asks verify-evidence.sh to certify; empty = its default (full).
TIER_FLAG=""

# check <name> <expected-exit> <grep-pattern-that-must-appear>
check() {
    local name="$1" want="$2" pattern="$3" out rc
    # shellcheck disable=SC2086 # TIER_FLAG is empty or two words, by design
    out="$(bash scripts/verify-evidence.sh --strict $TIER_FLAG 2>&1)"
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

# check_absent <name> <grep-pattern-that-must-NOT-appear>: the run is still
# rejected (exit 1), but without this message.
check_absent() {
    local name="$1" pattern="$2" out rc
    out="$(bash scripts/verify-evidence.sh --strict 2>&1)"
    rc=$?
    if [ "$rc" -ne 1 ] || printf '%s\n' "$out" | grep -q "$pattern"; then
        echo "  FAIL  $name: exit $rc, or a message matching '$pattern'"
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

# The full tier has one test leg: the build this repository ships composes no
# extension, so its no-extension suite runs in the default leg and no other
# feature set's leg is required. Its binaries are the whole denominator.
seed_all_valid
check "the one test leg is the full tier's whole denominator" 0 "10/10 test binaries"

# Evidence a retired leg left behind (an `agents` or no-extension run, even one
# that failed) is not read: only the gates the tier names are.
seed_all_valid
write_gate test-agents full fail "$TREE" "$(good_unit)"
write_gate test-noext full fail "$TREE" "$(good_unit)"
check "a retired leg's leftover evidence is ignored" 0 "10/10 test binaries"

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

# --- the public-terms gate (S-716) --------------------------------------------
#
# Required at BOTH tiers, and its counters are recomputed like a test unit's: an
# empty walk or a hit fails it whatever its verdict field says.

seed_all_valid
rm -f "$EVID/public-terms.json"
check "the public-terms gate is required at full tier" 1 "public-terms: no evidence file"

seed_all_valid fast
rm -f "$EVID/public-terms.json"
TIER_FLAG="--tier fast"
check "the public-terms gate is required at fast tier" 1 "public-terms: no evidence file"
TIER_FLAG=""

seed_all_valid
write_gate public-terms full pass "$TREE" "$(files_unit 0 0)"
check "a public-terms walk that read zero files rejected" 1 "public-terms/files: the walk read zero text files"

seed_all_valid
write_gate public-terms full pass "$TREE" "$(files_unit 1044 2)"
check "public-terms hits under a 'pass' verdict rejected" 1 "public-terms/files: 2 term hit(s)"

# Every refusal of the files unit speaks of files, never of test binaries.
seed_all_valid
write_gate public-terms full pass "$TREE" "$(files_unit 0 0)"
check_absent "an empty walk is not also misreported as a test-floor breach" "is below"

seed_all_valid
write_gate public-terms full pass "$TREE" '[{"unit":"files","expected":1044,"floor":1,"started":1044,"finished":900,"passed":900,"failed":0,"ignored":0,"exit_code":0,"verdict":"pass","truncation":"none"}]'
check "a killed public-terms walk is reported in files" 1 "public-terms/files: 1044 text files started, only 900 finished — the scan was killed mid-walk"

seed_all_valid
write_gate public-terms full pass "$TREE" '[{"unit":"files","expected":1044,"floor":1,"started":900,"finished":900,"passed":900,"failed":0,"ignored":0,"exit_code":0,"verdict":"pass","truncation":"none"}]'
check "a truncated public-terms walk is reported in files" 1 "public-terms/files: 900 of 1044 text files ran"

seed_all_valid
check "the public-terms walk is reported with its denominator, outside the test count" 0 "public-terms       full   pass        1044/1044 files 0 hits"

# --- the timing-assertions gate (S-804) ---------------------------------------
#
# Required at BOTH tiers (the check reads the whole tree, whatever the change
# touched), and its one unit counts the timing assertions the walk found: a walk
# that found none, or an upper bound that bypasses the helper, fails it whatever
# its verdict field says.

seed_all_valid
rm -f "$EVID/timing-assertions.json"
check "the timing-assertions gate is required at full tier" 1 "timing-assertions: no evidence file"

seed_all_valid fast
rm -f "$EVID/timing-assertions.json"
TIER_FLAG="--tier fast"
check "the timing-assertions gate is required at fast tier" 1 "timing-assertions: no evidence file"
TIER_FLAG=""

seed_all_valid
write_gate timing-assertions full pass "$TREE" "$(sites_unit 0 0)"
check "a timing-assertions walk that found no site rejected" 1 "timing-assertions/sites: the walk found no timing assertion"

seed_all_valid
write_gate timing-assertions full pass "$TREE" "$(sites_unit 71 2)"
check "unscaled bounds under a 'pass' verdict rejected" 1 "timing-assertions/sites: 2 unscaled elapsed-time bound(s)"

seed_all_valid
write_gate timing-assertions full pass "$TREE" "$(sites_unit 0 0)"
check_absent "an empty walk is not also misreported as a test-floor breach" "is below"

seed_all_valid
check "the timing-assertions walk is reported with its denominator, outside the test count" 0 "timing-assertions  full   pass        71/71 sites    0 unscaled"

# --- the offline-cargo gate (S-805) --------------------------------------------
#
# Required at BOTH tiers like the timing-assertions gate, and its one unit counts
# the offline cargo calls the walk found: a walk that found none, or a call naming
# no platform, fails it whatever its verdict field says.

seed_all_valid
rm -f "$EVID/offline-cargo.json"
check "the offline-cargo gate is required at full tier" 1 "offline-cargo: no evidence file"

seed_all_valid fast
rm -f "$EVID/offline-cargo.json"
TIER_FLAG="--tier fast"
check "the offline-cargo gate is required at fast tier" 1 "offline-cargo: no evidence file"
TIER_FLAG=""

seed_all_valid
write_gate offline-cargo full pass "$TREE" "$(cargo_unit 0 0)"
check "an offline-cargo walk that found no call rejected" 1 "offline-cargo/sites: the walk found no offline cargo call"

seed_all_valid
write_gate offline-cargo full pass "$TREE" "$(cargo_unit 4 1)"
check "an unfiltered call under a 'pass' verdict rejected" 1 "offline-cargo/sites: 1 unfiltered offline cargo call(s)"

seed_all_valid
check "the offline-cargo walk is reported with its denominator, outside the test count" 0 "offline-cargo      full   pass        4/4 sites      0 unfiltered"

# --- the agents-marker gate ------------------------------------------------------
#
# Required at the full tier only (gate.sh fast never runs it). Its counters are
# the gated tests, and they are recomputed like any unit's: a run in which the
# gated test never executed is rejected whatever its verdict field says.

seed_all_valid
rm -f "$EVID/agents-marker.json"
check "the agents-marker gate is required at full tier" 1 "agents-marker: no evidence file"

seed_all_valid
write_gate agents-marker full pass "$TREE" "$(gated_unit 1 0 0 0)"
check "an agents-marker run in which the gated test never ran rejected" 1 "agents-marker/gated: the agents-gated test did not execute"

seed_all_valid
write_gate agents-marker full pass "$TREE" "$(gated_unit 1 1 0 0)"
check "a gated test that started and never reported rejected" 1 "agents-marker/gated: 1 gated tests started, only 0 finished"

seed_all_valid
write_gate agents-marker full pass "$TREE" "$(gated_unit 2 1 1 0)"
check "fewer gated tests than the script names rejected" 1 "agents-marker/gated: 1 of 2 gated tests ran"

seed_all_valid
write_gate agents-marker full pass "$TREE" "$(gated_unit 1 1 1 1)"
check "a failed gated test under a 'pass' verdict rejected" 1 "agents-marker/gated: 1 test(s) failed"

seed_all_valid
check "the agents-marker gate is reported with its denominator" 0 "agents-marker      full   pass        1/1 gated"

# Exactly what `gate.sh fast` writes: its five gates and nothing else.
seed_all_valid fast
for g in agents-marker deny script-suites ui-typecheck ui-test ui-build ui-e2e; do rm -f "$EVID/$g.json"; done
TIER_FLAG="--tier fast"
check "genuinely valid fast-tier evidence accepted" 0 "EVIDENCE VERIFIED for tree $TREE (tier fast, 10/10 test binaries)"
TIER_FLAG=""

# --- the script-suites gate (S-719) ---------------------------------------------
#
# Required at the full tier, one unit per scripts/tests/test-*.sh the tree holds.
# `gate.sh fast` runs it exactly when scripts/ or the plugin tree changed, so the fast tier accepts
# evidence with or without it and validates it when it is there. A unit's exit
# status is read on its own, not only through `failed`.

seed_all_valid
rm -f "$EVID/script-suites.json"
check "the script-suites gate is required at full tier" 1 "script-suites: no evidence file"

seed_all_valid
check "the script-suites gate is reported with its denominator" 0 "script-suites   *full   pass        2/2 suites *19 checks passed"

seed_all_valid
write_gate script-suites full pass "$TREE" "[$(suite_unit test-alpha 12 0 0)]"
check "a suite the tree holds but the evidence has no unit for rejected" 1 "script-suites/test-beta: the tree holds this suite but the evidence has no unit for it"

seed_all_valid
write_gate script-suites full pass "$TREE" "[]"
check "script-suites passed over zero suites rejected" 1 "script-suites: passed with no suite unit"

seed_all_valid
write_gate script-suites full fail "$TREE" "[$(suite_unit test-alpha 12 0 0),$(suite_unit test-beta 5 2 1)]"
check "a failed suite unit rejected, with its failure count" 1 "script-suites/test-beta: 2 check(s) failed"

# The unit's exit status, with no counted failure and every counter green.
seed_all_valid
write_gate script-suites full pass "$TREE" "[$(suite_unit test-alpha 12 0 0),$(suite_unit test-beta 7 0 3)]"
check "a suite that exited non-zero under a 'pass' is rejected on its exit status" 1 "script-suites/test-beta: the suite exited 3"

seed_all_valid
write_gate script-suites full pass "$TREE" "[$(suite_unit test-alpha 12 0 0),$(suite_unit test-beta 0 0 0)]"
check "a suite that counted no passing check is not a pass" 1 "script-suites/test-beta: the suite counted no passing check"

seed_all_valid
write_gate script-suites full pass "$TREE" "[$(suite_unit test-alpha 12 0 0),$(suite_unit test-beta 7 0 0)]"
python3 - "$EVID/script-suites.json" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
d["units"][1]["finished"] = 0       # started, never finished: killed by a signal
json.dump(d, open(sys.argv[1], "w"))
PY
check "a suite killed by a signal is reported in suites" 1 "script-suites/test-beta: 1 suites started, only 0 finished — the suite was killed by a signal"

seed_all_valid
write_gate script-suites full pass "$TREE" "[$(suite_unit test-alpha 12 0 0),$(suite_unit test-beta 7 0 0)]"
python3 - "$EVID/script-suites.json" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
d["units"][1]["started"] = 0        # the unit never started (the gate's empty-roster unit)
d["units"][1]["finished"] = 0
json.dump(d, open(sys.argv[1], "w"))
PY
check "a suite unit that never started is reported in suites" 1 "script-suites/test-beta: the suite did not run"

seed_all_valid
write_gate script-suites fast pass "$TREE" "$(suites_valid)"
check "fast-tier script-suites evidence never satisfies a full handoff" 1 'script-suites: tier is .fast., not "full"'

# Fast tier: with or without the leg, and validated when present.
seed_all_valid fast
for g in agents-marker deny ui-typecheck ui-test ui-build ui-e2e; do rm -f "$EVID/$g.json"; done
TIER_FLAG="--tier fast"
check "fast tier accepts evidence WITH the script-suites leg" 0 "EVIDENCE VERIFIED for tree $TREE (tier fast"
mkdir -p "$WORK/pending"
bash scripts/verify-evidence.sh --strict --tier fast --write-marker "$WORK/pending/S-001-review-done" >/dev/null 2>&1
if grep -q "^EVIDENCE-SCOPE tier=fast gates=.*script-suites" "$WORK/pending/S-001-review-done" 2>/dev/null; then
    echo "  ok    the marker names the script-suites leg it certified"
    PASS=$((PASS + 1))
else
    echo "  FAIL  the marker does not name the script-suites leg: $(cat "$WORK/pending/S-001-review-done" 2>&1)"
    FAIL=$((FAIL + 1))
fi
check "fast tier lists the script-suites leg it validated" 0 "script-suites *fast   pass        2/2 suites"
rm -f "$EVID/script-suites.json"
check "fast tier accepts evidence WITHOUT the script-suites leg" 0 "EVIDENCE VERIFIED for tree $TREE (tier fast"
write_gate script-suites fast pass "$TREE" "[$(suite_unit test-alpha 12 0 0),$(suite_unit test-beta 5 2 1)]"
check "fast tier validates the leg when present: a failed suite rejected" 1 "script-suites/test-beta: 2 check(s) failed"
write_gate script-suites fast pass "$TREE" "[$(suite_unit test-alpha 12 0 0)]"
check "fast tier validates the leg when present: a suite left out rejected" 1 "script-suites/test-beta: the tree holds this suite"
write_gate script-suites fast fail "$TREE" "$(suites_valid)"
check "fast tier validates the leg when present: a failed verdict rejected" 1 "script-suites: verdict 'fail'"
# Evidence from an earlier tree is history, not a claim about this one.
write_gate script-suites fast fail "0000000000000000000000000000000000000000" "[$(suite_unit test-beta 5 2 1)]"
check "fast tier ignores script-suites evidence for an older tree" 0 "EVIDENCE VERIFIED for tree $TREE (tier fast"
TIER_FLAG=""

# --- tier ci (S-806): the container run's evidence, and only it ---------------
#
# gate.sh ci records three gates under their own names. `--tier ci` certifies tier
# "ci" and nothing else; the fast and full tiers never read a ci gate; and the
# evidence of one is no use to the other.

seed_ci_valid() {
    local tier="${1:-ci}"
    rm -rf "$EVID"
    write_gate ci-ui-build "$tier" pass "$TREE" "[]"
    write_gate ci-script-suites "$tier" pass "$TREE" "$(suites_valid)"
    write_gate ci-test "$tier" pass "$TREE" "$(good_unit)"
}

TIER_FLAG="--tier ci"
seed_ci_valid
check "tier ci: valid ci evidence accepted, with the test denominator" 0 "EVIDENCE VERIFIED for tree $TREE (tier ci, 10/10 test binaries)"
seed_ci_valid
rm -f "$EVID/ci-test.json"
check "tier ci: a missing ci-test gate is a gate that did not run" 1 "ci-test: no evidence file"
seed_ci_valid
rm -f "$EVID/ci-ui-build.json"
check "tier ci: a missing ci-ui-build gate is a gate that did not run" 1 "ci-ui-build: no evidence file"
seed_ci_valid
rm -f "$EVID/ci-script-suites.json"
check "tier ci: a missing ci-script-suites gate is a gate that did not run" 1 "ci-script-suites: no evidence file"
seed_ci_valid
write_gate ci-test ci skip "$TREE" "[]"
check "tier ci: a skipped ci gate is not a pass" 1 "ci-test: skipped"
seed_ci_valid
write_gate ci-test ci pass "$TREE" '[{"unit":"workspace","expected":10,"floor":9,"started":10,"finished":7,"passed":50,"failed":0,"ignored":0,"exit_code":0,"verdict":"pass","truncation":"none"}]'
check "tier ci: a truncated workspace run is rejected from its counters" 1 "ci-test/workspace: 10 binaries started, only 7 finished"
seed_ci_valid
write_gate ci-test ci pass "$TREE" '[{"unit":"workspace","expected":10,"floor":9,"started":10,"finished":10,"passed":50,"failed":2,"ignored":0,"exit_code":0,"verdict":"pass","truncation":"none"}]'
check "tier ci: failing tests in the container are rejected" 1 "ci-test/workspace: 2 test(s) failed"
seed_ci_valid
write_gate ci-script-suites ci pass "$TREE" "[$(suite_unit test-alpha 12 0 0),$(suite_unit test-beta 5 2 1)]"
check "tier ci: a failed suite in the container is rejected" 1 "ci-script-suites/test-beta"
seed_ci_valid
write_gate ci-script-suites ci pass "$TREE" "[$(suite_unit test-alpha 12 0 0)]"
check "tier ci: a suite the tree holds and the container did not run is rejected" 1 "ci-script-suites/test-beta: the tree holds this suite"
seed_ci_valid
write_gate ci-test ci pass "0000000000000000000000000000000000000000" "$(good_unit)"
check "tier ci: ci evidence for another tree is stale" 1 "stale evidence"

seed_ci_valid
write_gate ci-script-suites ci pass "$TREE" '[{"unit":"test-alpha","expected":1,"floor":1,"started":0,"finished":0,"passed":0,"failed":0,"ignored":0,"exit_code":0,"verdict":"pass","truncation":"none"},'"$(suite_unit test-beta 7 0 0)"']'
check "tier ci: a suite unit speaks of suites, not binaries" 1 "ci-script-suites/test-alpha: the suite did not run"

# Evidence of another tier is refused in its place.
seed_ci_valid fast
check "tier ci: ci gates that say tier fast are refused" 1 "ci-test: tier is 'fast', not \"ci\""
seed_ci_valid full
check "tier ci: ci gates that say tier full are refused" 1 "ci-test: tier is 'full', not \"ci\""
seed_all_valid
check "tier ci: a complete FULL-tier evidence set is refused" 1 "never stands in for tier ci"
seed_all_valid fast
check "tier ci: a FAST-tier evidence set is refused" 1 "ci-test: no evidence file"

# ... and ci evidence is no use to the other two tiers, whole or partial.
TIER_FLAG="--tier full"
seed_ci_valid
check "tier full: a complete ci evidence set is refused" 1 "clippy: no evidence file"
seed_all_valid
write_gate ci-test ci pass "$TREE" "$(good_unit)"
check "tier full: a ci gate beside valid full evidence is not read" 0 "EVIDENCE VERIFIED for tree $TREE (tier full"
TIER_FLAG="--tier fast"
seed_ci_valid
check "tier fast: a complete ci evidence set is refused" 1 "clippy: no evidence file"
seed_all_valid fast
write_gate ci-test ci fail "$TREE" "$(good_unit)"
check "tier fast: a failed ci gate beside valid fast evidence is not read" 0 "EVIDENCE VERIFIED for tree $TREE (tier fast"
# A full-tier gate under a ci gate's tier is no ci evidence either.
write_gate test ci pass "$TREE" "$(good_unit)"
check "tier fast: a fast gate that says tier ci is refused" 1 "test: tier is 'ci', which is neither"
TIER_FLAG=""

# The marker is a handoff's; a publish precondition writes none.
seed_ci_valid
mkdir -p "$WORK/pending"
bash scripts/verify-evidence.sh --strict --tier ci --write-marker "$WORK/pending/S-806-review-done" >/dev/null 2>&1
rc=$?
if [ "$rc" -eq 2 ] && [ ! -e "$WORK/pending/S-806-review-done" ]; then
    echo "  ok    tier ci writes no marker (exit 2, nothing written)"
    PASS=$((PASS + 1))
else
    echo "  FAIL  tier ci --write-marker: exit $rc, marker present: $([ -e "$WORK/pending/S-806-review-done" ] && echo yes || echo no)"
    FAIL=$((FAIL + 1))
fi
bash scripts/verify-evidence.sh --strict --tier bogus >/dev/null 2>&1
rc=$?
if [ "$rc" -eq 2 ]; then
    echo "  ok    an unknown tier is a usage error"
    PASS=$((PASS + 1))
else
    echo "  FAIL  an unknown tier exited $rc, wanted 2"
    FAIL=$((FAIL + 1))
fi

# --- --tree (S-807): certify a given tree, not the working directory's ------------
#
# scripts/publish-sprint.sh publishes local main's tree, wherever it is run from. A
# working directory that has moved on (an untracked file, a suite not committed)
# must neither make main's evidence stale nor lend its own suites to the check.

: >scripts/tests/test-gamma.sh
seed_ci_valid
TIER_FLAG="--tier ci"
check "--tree: a dirty working tree makes ci evidence for HEAD's tree stale without it" 1 "stale evidence"
TIER_FLAG="--tier ci --tree $TREE"
check "--tree: ci evidence for the tree given is accepted, the working directory notwithstanding" 0 "EVIDENCE VERIFIED for tree $TREE (tier ci"
seed_ci_valid
write_gate ci-test ci pass "0000000000000000000000000000000000000000" "$(good_unit)"
check "--tree: ci evidence for another tree than the one given is stale" 1 "stale evidence"
seed_ci_valid
TIER_FLAG="--tier ci --tree HEAD"
check "--tree: a revision that names the tree is resolved to it" 0 "EVIDENCE VERIFIED for tree $TREE"
TIER_FLAG="--tier ci --tree $TREE"
write_gate ci-script-suites ci pass "$TREE" "[$(suite_unit test-alpha 12 0 0)]"
check "--tree: a suite the given tree holds and the run lacks is still rejected" 1 "ci-script-suites/test-beta: the tree holds this suite"
rm -f scripts/tests/test-gamma.sh
# The suites a tree holds are the direct children of scripts/tests, as gate.sh runs
# them: a helper nested under it is not a suite, and a name with a non-ASCII byte is.
mkdir -p scripts/tests/fixtures
: >scripts/tests/fixtures/test-helper.sh
git add -A >/dev/null 2>&1
git commit -qm "nested helper" >/dev/null 2>&1
TREE_NESTED="$(git rev-parse 'HEAD^{tree}')"
seed_ci_valid
for g in ci-ui-build ci-script-suites ci-test; do sed -i.bak "s/$TREE/$TREE_NESTED/" "$EVID/$g.json"; done
rm -f "$EVID"/*.bak
TIER_FLAG="--tier ci --tree $TREE_NESTED"
check "--tree: a test-*.sh nested under scripts/tests is not a suite the tree holds" 0 "EVIDENCE VERIFIED for tree $TREE_NESTED"
: >"scripts/tests/test-é.sh"
git add -A >/dev/null 2>&1
git commit -qm "non-ascii suite" >/dev/null 2>&1
TREE_UTF="$(git rev-parse 'HEAD^{tree}')"
for g in ci-ui-build ci-script-suites ci-test; do sed -i.bak "s/$TREE_NESTED/$TREE_UTF/" "$EVID/$g.json"; done
rm -f "$EVID"/*.bak
TIER_FLAG="--tier ci --tree $TREE_UTF"
check "--tree: a suite with a non-ASCII name that the run lacks is still rejected" 1 "ci-script-suites/test-é: the tree holds this suite"
git reset -q --hard "$(git rev-list --max-parents=0 HEAD)" >/dev/null 2>&1
seed_ci_valid
TIER_FLAG=""
bash scripts/verify-evidence.sh --strict --tier ci --tree 0123456789abcdef0123456789abcdef01234567 >/dev/null 2>&1
rc=$?
if [ "$rc" -eq 2 ]; then
    echo "  ok    --tree naming no tree of the repository is a usage error"
    PASS=$((PASS + 1))
else
    echo "  FAIL  --tree naming no tree exited $rc, wanted 2"
    FAIL=$((FAIL + 1))
fi
for empty in '--tree' '--tree ""'; do
    eval "bash scripts/verify-evidence.sh --strict --tier ci $empty" >/dev/null 2>&1
    rc=$?
    if [ "$rc" -eq 2 ]; then
        echo "  ok    $empty with no tree id is a usage error, not the working directory's tree"
        PASS=$((PASS + 1))
    else
        echo "  FAIL  $empty with no tree id exited $rc, wanted 2"
        FAIL=$((FAIL + 1))
    fi
done
seed_all_valid
bash scripts/verify-evidence.sh --strict --tree "$TREE" --write-marker "$WORK/pending/S-807-review-done" >/dev/null 2>&1
rc=$?
if [ "$rc" -eq 2 ] && [ ! -e "$WORK/pending/S-807-review-done" ]; then
    echo "  ok    --tree writes no marker (exit 2, nothing written)"
    PASS=$((PASS + 1))
else
    echo "  FAIL  --tree --write-marker: exit $rc, marker present: $([ -e "$WORK/pending/S-807-review-done" ] && echo yes || echo no)"
    FAIL=$((FAIL + 1))
fi

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
