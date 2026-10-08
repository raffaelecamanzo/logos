#!/usr/bin/env bash
#
# test-ui-e2e-classify.sh — prove scripts/ui-e2e-classify.py turns every way the
# ui-e2e leg could read green while checking nothing into a failure (S-611).
#
# Each case crafts a Playwright report / exit status / log and asserts the exact
# line the classifier prints. The all-pass case asserts it says yes — without
# it, a classifier that failed unconditionally would pass this test.
#
# Usage: bash scripts/tests/test-ui-e2e-classify.sh
# Exit:  0 all cases behaved, 1 a case did not

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CLASSIFY="$HERE/scripts/ui-e2e-classify.py"
WORK="$HERE/scripts/tests/.ui-e2e-classify.tmp.$$"
trap 'rm -rf "$WORK"' EXIT INT TERM
mkdir -p "$WORK" || exit 1

PASS=0
FAIL=0

# check <name> <report-json-or-ABSENT> <rc> <log-text> <expected-line>
check() {
    local name="$1" report="$2" rc="$3" logtext="$4" want="$5" got
    rm -f "$WORK/report.json"
    [ "$report" = ABSENT ] || printf '%s' "$report" >"$WORK/report.json"
    printf '%s' "$logtext" >"$WORK/run.log"
    got="$(python3 "$CLASSIFY" "$WORK/report.json" "$rc" "$WORK/run.log")"
    if [ "$got" = "$want" ]; then
        echo "  ok    $name"
        PASS=$((PASS + 1))
    else
        echo "  FAIL  $name: got '$got', wanted '$want'"
        FAIL=$((FAIL + 1))
    fi
}

stats() { printf '{"stats":{"expected":%d,"unexpected":%d,"flaky":%d,"skipped":%d}}' "$@"; }

check "every test passed"          "$(stats 5 0 0 0)" 0 ""  "pass none 5 5 5 0 0"
check "one test failed"            "$(stats 4 1 0 0)" 1 ""  "fail none 5 5 4 1 0"
check "a flaky test is a failure"  "$(stats 4 0 1 0)" 0 ""  "fail none 5 5 4 1 0"
check "every test skipped"         "$(stats 0 0 0 5)" 0 ""  "fail no_binaries 5 0 0 0 5"
check "one test skipped"           "$(stats 4 0 0 1)" 0 ""  "fail skipped 5 4 4 0 1"
check "zero tests listed"          "$(stats 0 0 0 0)" 0 ""  "fail no_binaries 0 0 0 0 0"
check "no report written"          ABSENT             1 ""  "fail no_binaries 0 0 0 0 0"
check "report is not JSON"         "not json"         0 ""  "fail no_binaries 0 0 0 0 0"
check "report root is a list"      "[]"               0 ""  "fail no_binaries 0 0 0 0 0"
check "stats are not numbers"      '{"stats":{"expected":"x"}}' 0 "" "fail no_binaries 0 0 0 0 0"
check "stats are missing"          '{"suites":[]}'    0 ""  "fail no_binaries 0 0 0 0 0"
check "killed by the watchdog"     ABSENT             142 "" "fail killed 0 0 0 0 0"
check "browser not installed"      "$(stats 0 5 0 0)" 1 \
    "Error: browserType.launch: Executable doesn't exist at /x" "fail missing_browser 5 5 0 5 0"
check "clean report, non-zero exit" "$(stats 5 0 0 0)" 1 "" "fail none 5 5 5 0 0"

echo
echo "passed $PASS, failed $FAIL"
[ "$FAIL" -eq 0 ]
