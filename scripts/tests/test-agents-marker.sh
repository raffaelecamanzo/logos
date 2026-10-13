#!/usr/bin/env bash
#
# test-agents-marker.sh — prove the agents-marker leg cannot read green while the
# code behind logos-core's `agents` marker went untested (scripts/agents-marker.sh
# and gate.sh's gate_agents_marker).
#
# Two layers, each against a stub `cargo` / stub script so nothing is compiled:
#   1. scripts/agents-marker.sh over fixture cargo output — the run that passed,
#      and each way a run can look green while the gated test never executed (not
#      listed, a near-miss name, ignored, a build error, no binary, a killed
#      binary, a non-zero cargo exit under an all-ok log);
#   2. gate.sh's gate_agents_marker (cut out of it by name) over a stub script —
#      the evidence it writes: the unit's counters, the verdict, the truncation,
#      and a script that printed no denominator at all.
# Then a mutation phase: a one-line change to a copy of either script must turn
# the suite red on the case it targets. The real mutation of Surface::Chat's wire
# value is exercised by hand (it needs a compile); see the S-652 implementation
# notes.
#
# Usage: bash scripts/tests/test-agents-marker.sh
# Env:   GATE_SH, MARKER_SH  the scripts under test (default: scripts/gate.sh,
#        scripts/agents-marker.sh)
# Exit:  0 all cases behaved, 1 a case did not

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
GATE="${GATE_SH:-$HERE/scripts/gate.sh}"
MARKER="${MARKER_SH:-$HERE/scripts/agents-marker.sh}"
SELF="$HERE/scripts/tests/test-agents-marker.sh"

WORK="$HERE/scripts/tests/.agents-marker.tmp.$$"
trap 'rm -rf "$WORK"' EXIT INT TERM
mkdir -p "$WORK/bin" || exit 1

PASS=0
FAIL=0
ok() { echo "  ok    $1"; PASS=$((PASS + 1)); }
bad() { echo "  FAIL  $1"; FAIL=$((FAIL + 1)); }

GATED="observability::tests::chat_surface_calls_are_separable_from_web_and_mcp"

# --- layer 1: scripts/agents-marker.sh over fixture cargo output ---------------
#
# The stub cargo prints $FIXTURE_OUT (libtest's stdout) and $FIXTURE_ERR (cargo's
# stderr) and exits $FIXTURE_RC. The script reads both from one log, as cargo's
# two streams land in it.
cat >"$WORK/bin/cargo" <<'STUB'
#!/usr/bin/env bash
printf '%s\n' "$*" >"${FIXTURE_ARGS:-/dev/null}"
printf '%s' "${FIXTURE_ERR:-}" >&2
printf '%s' "${FIXTURE_OUT:-}"
exit "${FIXTURE_RC:-0}"
STUB
chmod +x "$WORK/bin/cargo"

RUNNING="     Running unittests src/lib.rs (target/debug/deps/logos_core-1)
"
RESULT_OK="test result: ok. 88 passed; 0 failed; 0 ignored; 0 measured; 2952 filtered out; finished in 0.5s
"
GATED_OK="test $GATED ... ok
"

# run <name> <rc> <err> <out> <expected exit> <needle in the last line>
run() {
    local name="$1" rc="$2" err="$3" out="$4" want="$5" needle="$6" got last
    got="$(PATH="$WORK/bin:$PATH" FIXTURE_ARGS="$WORK/cargo.args" FIXTURE_RC="$rc" FIXTURE_ERR="$err" FIXTURE_OUT="$out" \
        bash "$MARKER" --log "$WORK/run.log" 2>&1)"
    local code=$?
    last="$(printf '%s\n' "$got" | tail -n 1)"
    case "$last" in
        *"$needle"*) ;;
        *)
            bad "$name: last line '$last' lacks '$needle'"
            return
            ;;
    esac
    if [ "$code" -eq "$want" ]; then ok "$name"; else bad "$name: exit $code, wanted $want ($last)"; fi
}

run "[run] the gated test ran and the run passed" \
    0 "$RUNNING" "${GATED_OK}${RESULT_OK}" 0 "gated_expected=1 gated_started=1 gated_finished=1 passed=88 failed=0 ignored=0 rc=0 trunc=none"

# The headline property is the invocation itself: the stub cargo records its
# arguments, and a run is only worth anything if it asked for the marker, the lib
# tests of logos-core and the observability filter.
ARGS="$(cat "$WORK/cargo.args")"
for want in "test -p logos-core" "--lib" "--features agents" "observability::" "--no-fail-fast" "--test-threads=2"; do
    case "$ARGS" in
        *"$want"*) ok "[argv] cargo was invoked with '$want'" ;;
        *) bad "[argv] cargo was invoked without '$want': $ARGS" ;;
    esac
done

run "[gated] a run of 88 passes without the gated test is refused" \
    0 "$RUNNING" "test observability::tests::other ... ok
${RESULT_OK}" 1 "gated_finished=0 passed=88 failed=0 ignored=0 rc=0 trunc=gated_test_absent"

run "[gated] a near-miss test name does not stand in for the gated one" \
    0 "$RUNNING" "test ${GATED}_extra ... ok
${RESULT_OK}" 1 "trunc=gated_test_absent"

run "[gated] a prefix of the gated name does not stand in for it" \
    0 "$RUNNING" "test observability::tests::chat_surface ... ok
${RESULT_OK}" 1 "trunc=gated_test_absent"

run "[gated] a gated test that was ignored started and did not finish" \
    0 "$RUNNING" "test $GATED ... ignored
${RESULT_OK}" 1 "gated_started=1 gated_finished=0"

run "[gated] a gated test that failed is a failure, not an absence" \
    101 "$RUNNING" "test $GATED ... FAILED
test result: FAILED. 87 passed; 1 failed; 0 ignored; 0 measured; 2952 filtered out; finished in 0.5s
" 1 "gated_finished=1 passed=87 failed=1 ignored=0 rc=101"

run "[truncation] a build error executed nothing" \
    101 "error: could not compile \`logos-core\` (lib test) due to 1 previous error
" "" 1 "started=0 finished=0"

run "[truncation] no binary at all is no_binaries" \
    0 "" "" 1 "trunc=no_binaries"

run "[truncation] a binary that started and never summarised was killed" \
    0 "$RUNNING" "${GATED_OK}" 1 "started=1 finished=0 gated_expected=1 gated_started=1 gated_finished=1 passed=0 failed=0 ignored=0 rc=0 trunc=killed"

run "[exit] a non-zero cargo exit under an all-ok log is a failure" \
    1 "$RUNNING" "${GATED_OK}${RESULT_OK}" 1 "rc=1 trunc=none"

run "[truncation] cargo's own build error text is recorded as build_error" \
    101 "error: could not compile \`logos-core\` (lib test) due to 1 previous error
$RUNNING" "${GATED_OK}${RESULT_OK}" 1 "trunc=build_error"

# --- layer 2: gate.sh's gate_agents_marker -----------------------------------
extract() { # function-name -> its definition, from `name() {` to the first `^}` outside a heredoc
    awk -v n="$1" '
        $0 ~ "^"n"\\(\\) \\{" { p = 1 }
        p { print }
        p && /<<.?(PY|EOF).?$/ { h = 1; next }
        p && h && /^(PY|EOF)$/ { h = 0; next }
        p && !h && /^}/ { exit }
    ' "$GATE"
}
LEGFUNCS="$(for f in reset_units add_unit write_evidence gate_agents_marker; do extract "$f"; done)"
for f in reset_units add_unit write_evidence gate_agents_marker; do
    if ! printf '%s' "$LEGFUNCS" | grep -q "^$f()"; then
        echo "FAIL  could not extract $f from $GATE"
        exit 1
    fi
done

# leg <name> <script body>: run gate_agents_marker in a directory whose
# scripts/agents-marker.sh is the given stub; evidence lands in $WORK/<name>/.
leg() {
    local name="$1" body="$2"
    mkdir -p "$WORK/$name/scripts"
    printf '%s\n' "$body" >"$WORK/$name/scripts/agents-marker.sh"
    (
        cd "$WORK/$name" || exit 1
        set -uo pipefail
        EVID="$WORK/$name" TIER=full TREE_ID=t HEAD_SHA=h RUNNER=r WORKTREE=w
        eval "$LEGFUNCS"
        gate_done() { return 1; }
        record() { printf '  %s %s %s\n' "$1" "$2" "${3-}"; }
        gate_agents_marker
    )
}

# field <json> <python expression over the parsed record d>. The expressions are
# constants written in this file, evaluated by a throwaway interpreter on a record
# this suite just produced; no input from outside the repository reaches eval.
field() { python3 -I -c 'import json,sys; d=json.load(open(sys.argv[1])); print(eval(sys.argv[2]))' "$1" "$2"; }

GOOD='echo "cargo output"; echo "agents-marker: expected=1 started=1 finished=1 gated_expected=1 gated_started=1 gated_finished=1 passed=88 failed=0 ignored=0 rc=0 trunc=none"'
OUT="$(leg good "$GOOD")"
J="$WORK/good/agents-marker.json"
if [ "$(field "$J" "d['verdict']+' '+d['truncation']+' '+d['tier']")" = "pass none full" ]; then ok "[leg] a green script writes a passing full-tier record"; else bad "[leg] green record: $(cat "$J" 2>&1 | head -c 300)"; fi
if [ "$(field "$J" "[(u['unit'],u['expected'],u['started'],u['finished'],u['passed'],u['failed'])for u in d['units']]")" = "[('gated', 1, 1, 1, 88, 0)]" ]; then ok "[leg] its one unit carries the gated denominator and the run's pass count"; else bad "[leg] unit counters: $(field "$J" "d['units']")"; fi
case "$OUT" in *"agents-marker pass (1 of 1 gated tests ran; 88 passed, 0 failed, 0 ignored)"*) ok "[leg] the printed line names the denominator" ;; *) bad "[leg] printed: $OUT" ;; esac

ABSENT='echo "agents-marker: expected=1 started=1 finished=1 gated_expected=1 gated_started=0 gated_finished=0 passed=88 failed=0 ignored=0 rc=0 trunc=gated_test_absent"; exit 1'
leg absent "$ABSENT" >/dev/null
J="$WORK/absent/agents-marker.json"
if [ "$(field "$J" "d['verdict']+' '+d['truncation']+' '+str(d['units'][0]['started'])+' '+str(d['exit_code'])")" = "fail gated_test_absent 0 1" ]; then ok "[leg] a script that refused records the failure, its truncation and started=0"; else bad "[leg] absent record: $(cat "$J" 2>&1 | head -c 300)"; fi

# A script that exits 0 but whose summary reports a truncation is still a failure.
DISAGREE='echo "agents-marker: expected=1 started=1 finished=1 gated_expected=1 gated_started=1 gated_finished=1 passed=88 failed=0 ignored=0 rc=0 trunc=killed"'
leg disagree "$DISAGREE" >/dev/null
J="$WORK/disagree/agents-marker.json"
if [ "$(field "$J" "d['verdict']+' '+d['truncation']")" = "fail killed" ]; then ok "[leg] exit 0 under a reported truncation is still a failure"; else bad "[leg] disagree record: $(field "$J" "d['verdict']+' '+d['truncation']")"; fi

# The realistic failure: a gated test fails, so cargo exits 101 while the summary
# reports no truncation at all. Only the exit code distinguishes it from a pass.
FAILRUN='echo "agents-marker: expected=1 started=1 finished=1 gated_expected=1 gated_started=1 gated_finished=1 passed=87 failed=1 ignored=2 rc=101 trunc=none"; exit 1'
leg failrun "$FAILRUN" >/dev/null
J="$WORK/failrun/agents-marker.json"
if [ "$(field "$J" "d['verdict']+' '+d['truncation']+' '+str(d['exit_code'])")" = "fail none 1" ]; then ok "[leg] a failing gated test (exit 1, trunc=none) records a failure"; else bad "[leg] failrun record: $(field "$J" "d['verdict']+' '+d['truncation']+' '+str(d['exit_code'])")"; fi

# Distinct non-zero counters: a transposed pair in the unit would read the same
# from fixtures that report zero for both.
if [ "$(field "$WORK/failrun/agents-marker.json" "[(u['passed'],u['failed'],u['ignored']) for u in d['units']]")" = "[(87, 1, 2)]" ]; then ok "[leg] the unit records passed, failed and ignored in their own fields"; else bad "[leg] counters: $(field "$WORK/failrun/agents-marker.json" "d['units']")"; fi

# A summary line that lacks its `trunc=` key is unreadable, not clean.
NOKEY='echo "agents-marker: expected=1 started=1 finished=1 gated_expected=1 gated_started=1 gated_finished=1 passed=88 failed=0 ignored=0 rc=0"'
leg nokey "$NOKEY" >/dev/null
J="$WORK/nokey/agents-marker.json"
if [ "$(field "$J" "d['verdict']+' '+d['truncation']")" = "fail unreadable" ]; then ok "[leg] a summary without its trunc key records unreadable, not a pass"; else bad "[leg] nokey record: $(field "$J" "d['verdict']+' '+d['truncation']")"; fi

# A script that printed no denominator reported nothing: never a pass.
leg silent 'echo "cargo output only"' >/dev/null
J="$WORK/silent/agents-marker.json"
if [ "$(field "$J" "d['verdict']+' '+d['truncation']+' '+str(d['units'][0]['started'])")" = "fail no_summary 0" ]; then ok "[leg] a script with no denominator line records no_summary, not a pass"; else bad "[leg] silent record: $(field "$J" "d['verdict']+' '+d['truncation']")"; fi

# The leg is cut out of gate.sh by name, so nothing above proves the tiers CALL it:
# a deleted call would leave every case green while `gate.sh full` ran no such leg.
# Read each tier's branch of the closing `case "$TIER"` and require the call in
# `full)` and not in `fast)`.
tier_branch() { # fast|full -> that branch of the tier dispatch, up to its `;;`
    awk -v b="$1" '$0 ~ "^    " b "\\)$" { p = 1; next } p && /^        ;;$/ { exit } p { print }' "$GATE"
}
if tier_branch full | grep -q -x -F '        gate_agents_marker'; then ok "[wiring] the full tier calls the agents-marker leg"; else bad "[wiring] the full tier does not call gate_agents_marker"; fi
if tier_branch fast | grep -q -F 'gate_agents_marker'; then bad "[wiring] the fast tier calls the agents-marker leg"; else ok "[wiring] the fast tier does not call it"; fi
if [ -n "$(tier_branch fast)" ] && [ -n "$(tier_branch full)" ]; then ok "[wiring] both tier branches were found"; else bad "[wiring] a tier branch was not found in $GATE"; fi

# --- mutation phase ----------------------------------------------------------
#
# One line of a copy changed, the suite re-run against that copy. It must exit
# non-zero AND print a FAIL line carrying the tag of the case it targets. A target
# that is not in the script is itself a failure: a silent no-op mutation would read
# as a case that cannot fail.
NL=$'\n'
MUTATIONS=(
    "argv@@$MARKER@@--no-fail-fast --lib --features agents observability:: -- \\@@--no-fail-fast --features agents observability:: -- \\"
    "argv@@$MARKER@@--lib --features agents observability:: -- \\@@--lib observability:: -- \\"
    "argv@@$MARKER@@--features agents observability:: -- \\@@--features agents -- \\"
    "argv@@$MARKER@@cargo test -p logos-core --jobs 2@@cargo test --jobs 2"
    "argv@@$MARKER@@--no-fail-fast --lib@@--lib"
    "wiring@@$GATE@@        gate_agents_marker${NL}        gate_deny@@        gate_deny"
    "gated@@$MARKER@@elif [ \"\$gated_finished\" -lt \"\$gated_expected\" ]; then@@elif false; then"
    "gated@@$MARKER@@grep -q -x -F -e \"test \$t ... ok\" -e \"test \$t ... FAILED\" \"\$LOG\"@@grep -q -F -e \"test \$t\" \"\$LOG\""
    "truncation@@$MARKER@@elif [ \"\$started\" -gt \"\$finished\" ]; then@@elif false; then"
    "truncation@@$MARKER@@elif [ \"\$started\" -eq 0 ]; then@@elif false; then"
    "exit@@$MARKER@@elif [ \"\$failed\" -gt 0 ] || [ \"\$rc\" -ne 0 ]; then@@elif [ \"\$failed\" -gt 0 ]; then"
    "truncation@@$MARKER@@if grep -q -E '^error: (could not compile|none of the selected packages)' \"\$LOG\"; then@@if false; then"
    "leg@@$GATE@@if [ \"\$rc\" -ne 0 ] || [ \"\$trunc\" != none ]; then verdict=fail; fi@@if [ \"\$rc\" -ne 0 ]; then verdict=fail; fi"
    "leg@@$GATE@@if [ \"\$rc\" -ne 0 ] || [ \"\$trunc\" != none ]; then verdict=fail; fi@@if [ \"\$trunc\" != none ]; then verdict=fail; fi"
    "leg@@$GATE@@\"\$passed\" \"\$failed\" \"\$ignored\" \\@@\"\$passed\" \"\$ignored\" \"\$failed\" \\"
    "leg@@$GATE@@\"unreadable\" : v[\"trunc\"]@@\"none\" : v[\"trunc\"]"
    "leg@@$GATE@@trunc=no_summary@@trunc=none"
    "leg@@$GATE@@add_unit gated \"\$g_exp\" 1 \"\$g_start\" \"\$g_fin\"@@add_unit gated \"\$g_exp\" 1 \"\$g_exp\" \"\$g_exp\""
)

mutate() { # tag file old new
    local tag="$1" file="$2" old="$3" new="$4" copy="$WORK/mutant.sh" out rc env
    if ! python3 -I - "$file" "$copy" "$old" "$new" <<'PY'
import sys
src, dst, old, new = sys.argv[1:5]
text = open(src).read()
if text.count(old) != 1:
    sys.exit("mutation target occurs %d times, wanted exactly 1" % text.count(old))
open(dst, "w").write(text.replace(old, new))
PY
    then
        bad "[mutation:$tag] could not apply '$old'"
        return
    fi
    if [ "$file" = "$GATE" ]; then env="GATE_SH=$copy"; else env="MARKER_SH=$copy"; fi
    out="$(env "$env" AGENTS_MARKER_INNER=1 bash "$SELF" 2>&1)"
    rc=$?
    if [ "$rc" -ne 0 ] && printf '%s\n' "$out" | grep -q "FAIL  \[$tag\]"; then
        ok "[mutation:$tag] '$old' turns the suite red on its case"
    else
        bad "[mutation:$tag] '$old' left the suite green on its case (exit $rc)"
    fi
}

if [ -z "${AGENTS_MARKER_INNER:-}" ]; then
    for m in "${MUTATIONS[@]}"; do
        tag="${m%%@@*}"
        rest="${m#*@@}"
        file="${rest%%@@*}"
        rest="${rest#*@@}"
        old="${rest%%@@*}"
        new="${rest#*@@}"
        mutate "$tag" "$file" "$old" "$new"
    done
fi

echo "$PASS passed, $FAIL failed (of $((PASS + FAIL)))"
[ "$FAIL" -eq 0 ]
