#!/usr/bin/env bash
#
# test-run-bounded.sh — prove scripts/run-bounded.pl bounds a command AND its
# descendants (S-611). The case that matters is the second: a grandchild that
# outlives a plain `perl -e 'alarm …; exec …'` must not outlive this one.
#
# Usage: bash scripts/tests/test-run-bounded.sh
# Exit:  0 all cases behaved, 1 a case did not

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
RUN="$HERE/scripts/run-bounded.pl"
PASS=0
FAIL=0

expect() { # expect <name> <wanted> <got>
    if [ "$2" = "$3" ]; then
        echo "  ok    $1"
        PASS=$((PASS + 1))
    else
        echo "  FAIL  $1: got '$3', wanted '$2'"
        FAIL=$((FAIL + 1))
    fi
}

perl "$RUN" 10 sh -c 'exit 3'
expect "a command's own exit status passes through" 3 $?

# A unique duration names this test's sleeps, so pgrep sees only them.
MARK=$((4000 + $$ % 900))
perl "$RUN" 2 sh -c "sleep $MARK & sleep $MARK; wait" 2>/dev/null
expect "a command past its limit exits 142" 142 $?
sleep 1
if pgrep -f "sleep $MARK" >/dev/null 2>&1; then
    pkill -f "sleep $MARK"
    expect "no descendant survives the timeout" none survived
else
    expect "no descendant survives the timeout" none none
fi

perl "$RUN" 2>/dev/null
expect "no arguments is a usage error" 2 $?

perl "$RUN" 5 /nonexistent/command 2>/dev/null
expect "an unstartable command exits 127" 127 $?

echo
echo "passed $PASS, failed $FAIL"
[ "$FAIL" -eq 0 ]
