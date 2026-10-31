#!/usr/bin/env bash
#
# test-check-timing-assertions.sh — prove scripts/check-timing-assertions.py fails
# an elapsed-time assertion that bypasses the tolerance helper, and only that
# (S-804, NFR-MA-11).
#
# Each case copies fixture files (scripts/tests/fixtures/timing-assertions/*.rs.fixture,
# named so no tool reads them as source) into a throwaway root and runs the check
# there. The pinned shapes, in the order the story states them:
#   - HF-11's original assertion (a raw `elapsed < Duration::from_millis(200)`)  caught
#   - the same assertion through the helper                                       passes
#   - a loop guard (`while`/`if` on a deadline, no assertion)                     passes
#   - a multi-line `assert!`, with a coverage exemption                           caught
# and the controls around them: a liveness deadline built with and without the
# helper, a lower bound, names one step from the helper's, spellings of an upper
# bound that a line grep misses, text that is not code, and the empty walk. A check
# that flagged everything would catch every bypass, so each "passes" case reads its
# denominator too: the site was seen and judged, not skipped.
#
# Usage: bash scripts/tests/test-check-timing-assertions.sh
# Exit:  0 all cases behaved, 1 a case did not

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CHECK="$HERE/scripts/check-timing-assertions.py"
FIXTURES="$HERE/scripts/tests/fixtures/timing-assertions"

if [ ! -f "$CHECK" ] || [ ! -d "$FIXTURES" ]; then
    echo "cannot find scripts/check-timing-assertions.py and its fixtures (looked in $HERE)" >&2
    exit 1
fi

WORK="${TMPDIR:-/tmp}"
WORK="${WORK%/}/logos-timing-assertions-test.$$"
trap 'rm -rf "$WORK"' EXIT INT TERM
mkdir -p "$WORK" || exit 1

PASS=0
FAIL=0
N=0

# root <fixture-name>... -> sets ROOT: a fresh tree holding each fixture as
# src/<name>.rs
root() {
    N=$((N + 1))
    ROOT="$WORK/root$N"
    mkdir -p "$ROOT/src"
    for name in "$@"; do
        cp "$FIXTURES/$name.rs.fixture" "$ROOT/src/$name.rs"
    done
}

# run -> sets OUT and RC
run() {
    OUT="$(python3 "$CHECK" --root "$ROOT" 2>&1)"
    RC=$?
}

# expect <name> <wanted-exit> <summary line that must appear> [<text that must appear>]
expect() {
    local name="$1" want="$2" summary="$3" must="${4:-}"
    if [ "$RC" -ne "$want" ]; then
        echo "  FAIL  $name: exit $RC, wanted $want"
        printf '%s\n' "$OUT" | sed 's/^/        /'
        FAIL=$((FAIL + 1))
        return
    fi
    if ! printf '%s\n' "$OUT" | grep -qxF -- "$summary"; then
        echo "  FAIL  $name: exit $RC correct, but no line reading '$summary'"
        printf '%s\n' "$OUT" | sed 's/^/        /'
        FAIL=$((FAIL + 1))
        return
    fi
    if [ -n "$must" ] && ! printf '%s\n' "$OUT" | grep -qF -- "$must"; then
        echo "  FAIL  $name: no output naming '$must'"
        printf '%s\n' "$OUT" | sed 's/^/        /'
        FAIL=$((FAIL + 1))
        return
    fi
    echo "  ok    $name"
    PASS=$((PASS + 1))
}

echo "test-check-timing-assertions.sh"
echo "  work $WORK"
echo

# --- the four shapes the story pins --------------------------------------------

root hf11-original
run
expect "HF-11's original assertion is caught, with file, line and bound" 1 \
    "timing-assertions: files=1 sites=1 scaled=0 lower=0 hits=1" \
    "src/hf11-original.rs:12:"

root hf11-helper-form
run
expect "the same assertion through the helper passes" 0 \
    "timing-assertions: files=1 sites=1 scaled=1 lower=0 hits=0"

root loop-guard
run
expect "a loop guard asserts no bound: it is no site, and a walk of only guards is an empty walk" 3 \
    "timing-assertions: files=1 sites=0 scaled=0 lower=0 hits=0"

root loop-guard hf11-helper-form
run
expect "a loop guard beside a scaled site passes, and adds no site" 0 \
    "timing-assertions: files=2 sites=1 scaled=1 lower=0 hits=0"

root multi-line-assert
run
expect "a multi-line assert! behind a coverage exemption is caught at the macro's line" 1 \
    "timing-assertions: files=1 sites=1 scaled=0 lower=0 hits=1" \
    "src/multi-line-assert.rs:9:"

root multi-line-scaled
run
expect "the multi-line assert!, scaled, passes" 0 \
    "timing-assertions: files=1 sites=1 scaled=1 lower=0 hits=0"

# --- a liveness timeout carries its duration in the deadline --------------------

root deadline-unscaled
run
expect "assert!(Instant::now() < deadline) over an unscaled deadline is caught" 1 \
    "timing-assertions: files=1 sites=1 scaled=0 lower=0 hits=1" \
    "src/deadline-unscaled.rs:9:"

root deadline-scaled
run
expect "the same deadline built through the helper passes" 0 \
    "timing-assertions: files=1 sites=1 scaled=1 lower=0 hits=0"

# --- what is a bound, and what is the helper ------------------------------------

root lower-bound
run
expect "a lower bound is a site and never a hit; the upper bound beside it is scaled" 0 \
    "timing-assertions: files=1 sites=3 scaled=1 lower=2 hits=0"

root near-miss-names
run
expect "rescaled(, unscaled(, tolerance_ms( and a look-alike identifier are not the helper" 1 \
    "timing-assertions: files=1 sites=4 scaled=0 lower=0 hits=4"

root bypass-shapes
run
expect "as_millis, a bound bound to a name, the reversed and negated forms, and debug_assert! are caught" 1 \
    "timing-assertions: files=1 sites=5 scaled=0 lower=0 hits=5"

root scaled-shapes
run
expect "every one of those spellings passes once scaled" 0 \
    "timing-assertions: files=1 sites=5 scaled=5 lower=0 hits=0"

root not-code
run
expect "comments, strings, raw strings and char literals are not read" 0 \
    "timing-assertions: files=1 sites=1 scaled=1 lower=0 hits=0"

root ratios-and-eq
run
expect "a ratio of two measurements, assert_eq! and a non-time bound are no sites" 3 \
    "timing-assertions: files=1 sites=0 scaled=0 lower=0 hits=0"

# --- the spellings a reading of text can get wrong --------------------------------

root typed-let
run
expect "a typed let names a type: neither name nor scaling leaks from it" 1 \
    "timing-assertions: files=1 sites=3 scaled=1 lower=0 hits=2" \
    "src/typed-let.rs:7:"

root no-name-collisions
run
expect "a field, a call taking an instant, and an unrelated tuple element are no timing names" 3 \
    "timing-assertions: files=1 sites=0 scaled=0 lower=0 hits=0"

root tuple-positions
run
expect "a tuple let gives each name its own element's source" 1 \
    "timing-assertions: files=1 sites=1 scaled=0 lower=0 hits=1" \
    "src/tuple-positions.rs:6:"

root deadline-from-instant-unscaled
run
expect "a deadline built from an earlier instant, either way round, is caught" 1 \
    "timing-assertions: files=1 sites=2 scaled=0 lower=0 hits=2"

root deadline-from-instant-scaled
run
expect "the same deadlines scaled where they are built pass" 0 \
    "timing-assertions: files=1 sites=2 scaled=2 lower=0 hits=0"

root deadline-param
run
expect "an Instant parameter is scaled by its caller; a Duration parameter is not" 1 \
    "timing-assertions: files=1 sites=2 scaled=1 lower=0 hits=1" \
    "src/deadline-param.rs:12:"

root duration-bounds
run
expect "a duration field, a millisecond constant and a range's upper end are caught" 1 \
    "timing-assertions: files=1 sites=3 scaled=0 lower=0 hits=3"

root duration-bounds-scaled
run
expect "the same bounds scaled pass" 0 \
    "timing-assertions: files=1 sites=3 scaled=3 lower=0 hits=0"

root negated-group
run
expect "a negated group is read the other way round" 1 \
    "timing-assertions: files=1 sites=2 scaled=1 lower=0 hits=1" \
    "src/negated-group.rs:5:"

root assignment-bound
run
expect "a name assigned from an elapsed time, and one taken from it, are timing" 1 \
    "timing-assertions: files=1 sites=1 scaled=0 lower=0 hits=1"

root raw-strings
run
expect "raw and byte strings with odd inner quotes do not swallow the assertion after them" 1 \
    "timing-assertions: files=1 sites=1 scaled=0 lower=0 hits=1" \
    "src/raw-strings.rs:8:"

# --- the walk ------------------------------------------------------------------

root hf11-original hf11-helper-form multi-line-scaled
run
expect "one bypass among scaled sites fails the tree and names only the bypass" 1 \
    "timing-assertions: files=3 sites=3 scaled=2 lower=0 hits=1" \
    "src/hf11-original.rs:12:"
if printf '%s\n' "$OUT" | grep -q "hf11-helper-form.rs:.*not scaled\|multi-line-scaled.rs:.*not scaled"; then
    echo "  FAIL  the report names a scaled site"
    FAIL=$((FAIL + 1))
fi

N=$((N + 1))
ROOT="$WORK/empty"
mkdir -p "$ROOT/src" "$ROOT/target"
printf 'fn a() {}\n' >"$ROOT/target/skipped.rs"
run
expect "a tree with no assertion is an empty walk, and target/ is not walked" 3 \
    "timing-assertions: files=0 sites=0 scaled=0 lower=0 hits=0"

N=$((N + 1))
ROOT="$WORK/skipped"
mkdir -p "$ROOT/.worktrees/other" "$ROOT/node_modules/pkg" "$ROOT/.git"
for d in .worktrees/other node_modules/pkg .git; do
    cp "$FIXTURES/hf11-original.rs.fixture" "$ROOT/$d/bypass.rs"
done
run
expect "another checkout under .worktrees/, node_modules/ and .git/ are not walked" 3 \
    "timing-assertions: files=0 sites=0 scaled=0 lower=0 hits=0"

N=$((N + 1))
ROOT="$WORK/no-such-root"
run
expect "a root that is not a directory is a usage error" 2 \
    "check-timing-assertions.py: not a directory: $ROOT"

# The tree this check guards: every site in it already scales, and the walk
# read real files and real sites, not an empty set.
ROOT="$HERE"
run
if [ "$RC" -eq 0 ] && printf '%s\n' "$OUT" | grep -Eq '^timing-assertions: files=[1-9][0-9]* sites=[1-9][0-9]* scaled=[0-9]+ lower=[0-9]+ hits=0$'; then
    echo "  ok    this repository's timing assertions all scale through the helper"
    PASS=$((PASS + 1))
else
    echo "  FAIL  this repository's timing assertions: exit $RC"
    printf '%s\n' "$OUT" | sed 's/^/        /'
    FAIL=$((FAIL + 1))
fi

echo
echo "passed $PASS, failed $FAIL"
[ "$FAIL" -eq 0 ] || exit 1
exit 0
