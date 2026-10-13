#!/usr/bin/env bash
#
# agents-marker.sh — run the unit tests that cover code gated behind logos-core's
# `agents` marker, and refuse to call the run green unless the gated test ran.
#
# WHY THIS EXISTS
#   `agents` is a marker feature (logos-core/Cargo.toml): it adds no dependency and
#   declares `observability::Surface::Chat`, the override-only surface a composing
#   build attributes its engine calls to. No crate in this workspace turns it on,
#   so `cargo test --workspace` compiles that variant, its wire value and the
#   tests that name it out. This script is the one place they are compiled in. A
#   run that executed nothing summarises like a run that passed, so the verdict
#   rests on a denominator: the cargo binary must have started and finished, and
#   every test in GATED_TESTS must have printed its own result line.
#
# WHAT IT RUNS
#   cargo test -p logos-core --lib --features agents observability::
#   (--jobs 2 / --test-threads=2 / --no-fail-fast added: the same bounds the test
#   gate uses, see scripts/gate.sh run_test_unit).
#
# Usage:
#   bash scripts/agents-marker.sh [--log FILE]
#
#   The cargo output goes to FILE (default: a file under the git directory) and is
#   echoed after the run. The LAST line printed is the denominator, one line of
#   key=value pairs, which scripts/gate.sh reads by name:
#     agents-marker: expected=1 started=1 finished=1 gated_expected=1
#       gated_started=1 gated_finished=1 passed=88 failed=0 ignored=0 rc=0 trunc=none
#
# Exit codes:
#   0  the run passed and the gated test ran
#   1  the run failed, was truncated, or ran without the gated test
#   2  usage error / not a git repository

set -uo pipefail

# The tests that exist only with `agents` on. The exact libtest path of each: a
# prefix or substring match would admit a near miss that the marker does not gate.
GATED_TESTS=(
    "observability::tests::chat_surface_calls_are_separable_from_web_and_mcp"
)

LOG=""
while [ $# -gt 0 ]; do
    case "$1" in
        --log)
            [ $# -ge 2 ] || { echo "agents-marker.sh: --log needs a file" >&2; exit 2; }
            LOG="$2"
            shift 2
            ;;
        *)
            echo "usage: bash scripts/agents-marker.sh [--log FILE]" >&2
            exit 2
            ;;
    esac
done

ROOT="$(git rev-parse --show-toplevel 2>/dev/null)" || {
    echo "agents-marker.sh: not inside a git repository" >&2
    exit 2
}
cd "$ROOT" || exit 2
if [ -z "$LOG" ]; then
    LOG="$(git rev-parse --path-format=absolute --git-path agents-marker.log)"
fi

# Exactly one lib binary runs under --lib. With `started == finished >= 1` already
# required below, a run that finished fewer than this cannot get past those checks,
# so the figure is reported as the denominator and needs no branch of its own.
EXPECTED=1

CARGO_TERM_COLOR=never RAYON_NUM_THREADS=2 \
    cargo test -p logos-core --jobs 2 --no-fail-fast --lib --features agents observability:: -- \
    --test-threads=2 >"$LOG" 2>&1
rc=$?
cat "$LOG"

# `Running` is cargo's stderr, `test result:` libtest's stdout; both are in the one
# file, in order. Same counters scripts/gate.sh run_test_unit takes.
started="$(grep -c -E '^[[:space:]]*Running[[:space:]]' "$LOG")"
finished="$(grep -c -E '^test result: (ok|FAILED)\.' "$LOG")"
read -r passed failed ignored <<EOF
$(awk '/^test result: (ok|FAILED)\./ {
         for (i = 1; i <= NF; i++) {
           if ($(i+1) == "passed;")  p += $i
           if ($(i+1) == "failed;")  f += $i
           if ($(i+1) == "ignored;") g += $i
         }
       }
       END { printf "%d %d %d\n", p, f, g }' "$LOG")
EOF

# One result line per gated test, matched whole: `test <path> ... ok`.
gated_started=0
gated_finished=0
for t in "${GATED_TESTS[@]}"; do
    if grep -q -x -F -e "test $t ... ok" -e "test $t ... FAILED" "$LOG"; then
        gated_started=$((gated_started + 1))
        gated_finished=$((gated_finished + 1))
    elif grep -q -E "^test $t \.\.\." "$LOG"; then
        gated_started=$((gated_started + 1))
    fi
done
gated_expected="${#GATED_TESTS[@]}"

trunc=none
verdict=pass
if grep -q -E '^error: (could not compile|none of the selected packages)' "$LOG"; then
    trunc=build_error
    verdict=fail
elif [ "$started" -eq 0 ]; then
    trunc=no_binaries
    verdict=fail
elif [ "$started" -gt "$finished" ]; then
    trunc=killed
    verdict=fail
elif [ "$gated_finished" -lt "$gated_expected" ]; then
    trunc=gated_test_absent
    verdict=fail
elif [ "$failed" -gt 0 ] || [ "$rc" -ne 0 ]; then
    verdict=fail
fi

printf 'agents-marker: expected=%d started=%d finished=%d gated_expected=%d gated_started=%d gated_finished=%d passed=%d failed=%d ignored=%d rc=%d trunc=%s\n' \
    "$EXPECTED" "$started" "$finished" "$gated_expected" "$gated_started" "$gated_finished" \
    "$passed" "$failed" "$ignored" "$rc" "$trunc"
[ "$verdict" = pass ]
