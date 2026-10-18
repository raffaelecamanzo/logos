#!/usr/bin/env bash
#
# test-gate-script-suites.sh — prove scripts/gate.sh's script-suites leg (S-719,
# CR-227, NFR-CR-04) reports what the suites did, and runs exactly when it should.
#
# gate.sh runs when executed, so it cannot be sourced; the functions involved are
# cut out of it by name and run here against FIXTURES. Four layers:
#   1. suite_counts — the summary line of each shape this repository's suites print,
#      and the near misses that must not be read as one;
#   2. scripts_changed — the fast tier's condition, over a throwaway git repository:
#      a change under scripts/ (committed, uncommitted, non-ASCII, past the pipe
#      buffer) or under the plugin tree (plugins/, .claude-plugin/, S-678) runs the
#      leg; none, or a sibling directory called scripts-old or plugins-old, does
#      not; a base git cannot resolve fails closed;
#   3. gate_script_suites — the leg's verdict, over fixture suites: a green suite
#      passes; a red one, one that exits 0 having counted nothing, one that exits
#      non-zero without counting a failure, one killed by a signal, and an empty
#      roster each fail, and the unit counters say so;
#   4. the tier block itself — `full` runs the leg always, `fast` exactly when
#      scripts_changed says so (a plugin-only change included), by running the
#      real tier `case` with every gate replaced by a recorder.
# CI runs this suite beside the others (.github/workflows/ci.yml), and the leg runs
# it: the gate's own tests are guarded by the gate they test.
#
# Usage: bash scripts/tests/test-gate-script-suites.sh
# Env:   GATE_SH  path of the gate script under test (default: scripts/gate.sh)
# Exit:  0 all cases behaved, 1 a case did not

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
GATE="${GATE_SH:-$HERE/scripts/gate.sh}"

WORK="${TMPDIR:-/tmp}/logos-gate-script-suites-test.$$"
trap 'rm -rf "$WORK"' EXIT INT TERM
mkdir -p "$WORK" || exit 1
# A fixture repository is never inside any enclosing work tree.
export GIT_CEILING_DIRECTORIES="$WORK"

PASS=0
FAIL=0
ok() { echo "  ok    $1"; PASS=$((PASS + 1)); }
bad() { echo "  FAIL  $1"; FAIL=$((FAIL + 1)); }

extract() { # function-name -> its definition, from `name() {` to the first `^}` outside a heredoc
    awk -v n="$1" '
        $0 ~ "^"n"\\(\\) \\{" { p = 1 }
        p { print }
        p && /<<.?PY.?$/ { h = 1; next }
        p && h && /^PY$/ { h = 0; next }
        p && !h && /^}/ { exit }
    ' "$GATE"
}
FUNCS=""
for f in suite_counts gate_base scripts_changed reset_units add_unit write_evidence gate_script_suites; do
    def="$(extract "$f")"
    if [ -z "$def" ]; then
        echo "FAIL  could not extract $f from $GATE"
        exit 1
    fi
    FUNCS="$FUNCS
$def"
done

# expect <name> <got> <want>
expect() {
    if [ "$2" = "$3" ]; then ok "$1"; else bad "$1: got '$2', wanted '$3'"; fi
}

# --- 1. suite_counts ----------------------------------------------------------

counts() { # a log's text -> what suite_counts reads from it
    printf '%s\n' "$1" >"$WORK/counts.log"
    (eval "$FUNCS"; suite_counts "$WORK/counts.log")
}

expect "[counts] 'N passed, M failed (of K)' (the agents-marker and pkg-features shape)" \
    "$(counts '  ok    a
47 passed, 2 failed (of 49)')" "47 2"
expect "[counts] 'passed N, failed M' (the term-gate and verifier shape)" \
    "$(counts 'passed 36, failed 0')" "36 0"
expect "[counts] a '<name>: N passed, M failed' line" \
    "$(counts 'test-x: 7 passed, 1 failed')" "7 1"
expect "[counts] the LAST summary wins over an earlier one" \
    "$(counts '1 passed, 5 failed
passed 9, failed 0')" "9 0"
expect "[counts] the LAST of two 'N passed, M failed' lines wins" \
    "$(counts '1 passed, 5 failed
9 passed, 0 failed')" "9 0"
expect "[counts] the LAST of two 'passed N, failed M' lines wins" \
    "$(counts 'passed 1, failed 5
passed 9, failed 0')" "9 0"
expect "[counts] a trailing field after 'passed N, failed M,' is not part of the count" \
    "$(counts 'passed 3, failed 0, total 3')" "3 0"
expect "[counts] a count with letters glued on is not a summary (near miss)" \
    "$(counts 'passed 3, failed 0x')" ""
expect "[counts] a non-numeric token before 'failed' is not a count" \
    "$(counts 'x: 3 passed, 0 failed (2 failed in rerun)')" "3 0"
expect "[counts] no summary line reads as nothing" \
    "$(counts 'all good, trust me')" ""
expect "[counts] a summary-shaped fragment inside prose is not a summary (near miss)" \
    "$(counts 'the run: 3 passed, 0 failed? no — see below')" ""
expect "[counts] 'bypassed 3, failed 0' is not a summary (near miss)" \
    "$(counts 'bypassed 3, failed 0')" ""
expect "[counts] a leading-space line is not a summary (near miss)" \
    "$(counts '  passed 5, failed 0')" ""

# --- 2. scripts_changed -------------------------------------------------------
#
# Each case builds its own change on a branch off `base`; GATE_BASE names the base
# so the real gate_base is exercised through its override and the real git diff
# through the real function.

REPO="$WORK/repo"
mkdir -p "$REPO"
(
    cd "$REPO" || exit 1
    git init -q . >/dev/null 2>&1
    git config user.email t@example.com
    git config user.name test
    git config maintenance.auto false
    git config gc.auto 0
    mkdir -p scripts src
    echo a >scripts/a.sh
    echo b >src/b.rs
    git add -A >/dev/null 2>&1
    git commit -qm base >/dev/null 2>&1
    git tag base
) || { echo "FAIL  could not build the fixture repository"; exit 1; }

changed() { # name want-exit  (the repo is left as the caller made it)
    local name="$1" want="$2" rc
    (cd "$REPO" && eval "$FUNCS" && GATE_BASE="${GB:-base}" scripts_changed) >/dev/null 2>&1
    rc=$?
    if [ "$rc" -eq "$want" ]; then ok "$name"; else bad "$name: exit $rc, wanted $want"; fi
}
reset_repo() { (cd "$REPO" && git reset -q --hard base && git clean -qfd) >/dev/null 2>&1; }

GB=base
reset_repo
changed "[condition] nothing changed against the base: the leg does not run" 1

echo more >>"$REPO/src/b.rs"
(cd "$REPO" && git commit -qam "touch src" >/dev/null 2>&1)
changed "[condition] a change outside scripts/ does not run the leg" 1

reset_repo
echo more >>"$REPO/scripts/a.sh"
changed "[condition] an UNCOMMITTED change under scripts/ runs the leg" 0

(cd "$REPO" && git commit -qam "touch scripts" >/dev/null 2>&1)
changed "[condition] a COMMITTED change under scripts/ runs the leg (the handoff commits first)" 0

reset_repo
mkdir -p "$REPO/scripts/tests"
echo t >"$REPO/scripts/tests/test-new.sh"
(cd "$REPO" && git add -A >/dev/null 2>&1 && git commit -qm "new suite" >/dev/null 2>&1)
changed "[condition] a NEW file under scripts/ runs the leg" 0

reset_repo
mkdir -p "$REPO/scripts-old"
echo x >"$REPO/scripts-old/x.sh"
(cd "$REPO" && git add -A >/dev/null 2>&1 && git commit -qm "sibling dir" >/dev/null 2>&1)
changed "[condition] scripts-old/ is not scripts/ (near miss)" 1

reset_repo
mkdir -p "$REPO/src/scripts"
echo x >"$REPO/src/scripts/x.sh"
(cd "$REPO" && git add -A >/dev/null 2>&1 && git commit -qm "nested dir" >/dev/null 2>&1)
changed "[condition] src/scripts/ is not the top-level scripts/ (near miss)" 1

# scripts/ is not the first path of the diff: the match has to find it mid-list.
reset_repo
mkdir -p "$REPO/docs"
echo d >"$REPO/docs/d.md"
echo more >>"$REPO/scripts/a.sh"
(cd "$REPO" && git add -A >/dev/null 2>&1 && git commit -qm "docs then scripts" >/dev/null 2>&1)
changed "[condition] scripts/ among other changed paths, not first, runs the leg" 0

# A sibling directory named scripts-old/ AFTER another path must not match either.
reset_repo
mkdir -p "$REPO/docs" "$REPO/scripts-old"
echo d >"$REPO/docs/d.md"
echo x >"$REPO/scripts-old/x.sh"
(cd "$REPO" && git add -A >/dev/null 2>&1 && git commit -qm "docs then scripts-old" >/dev/null 2>&1)
changed "[condition] scripts-old/ after another changed path is still not scripts/ (near miss)" 1

# A file moved OUT of scripts/: rename detection would list only its new path.
reset_repo
(cd "$REPO" && git mv scripts/a.sh moved-out.sh >/dev/null 2>&1 && git commit -qm "move out" >/dev/null 2>&1)
changed "[condition] a file renamed out of scripts/ runs the leg (no rename detection)" 0

reset_repo
printf 'x\n' >"$REPO/scripts/é.sh"
(cd "$REPO" && git add -A >/dev/null 2>&1 && git commit -qm "non-ascii" >/dev/null 2>&1)
changed "[condition] a non-ASCII path under scripts/ runs the leg (quotePath off)" 0

# `git diff --name-only | grep -q` under pipefail exits 141 once grep stops early on
# a diff longer than the pipe buffer, which reads as "no change". scripts/ sorts
# FIRST here, ahead of ~150 KB of other paths.
reset_repo
echo more >>"$REPO/scripts/a.sh"
mkdir -p "$REPO/zzzzzzzz"
i=0
while [ $i -lt 3000 ]; do
    : >"$REPO/zzzzzzzz/a-rather-long-file-name-to-fill-the-pipe-buffer-$i.txt"
    i=$((i + 1))
done
(cd "$REPO" && git add -A >/dev/null 2>&1 && git commit -qm "big diff" >/dev/null 2>&1)
changed "[condition] a diff far past the pipe buffer, scripts/ first, still runs the leg" 0

reset_repo
GB=no-such-ref
changed "[condition] a base git cannot resolve fails closed: the leg runs" 0
GB=base

# The plugin tree (S-678): its scripts are tested by scripts/tests/test-plugin-*.sh,
# so a change to the plugin ALONE must run the suites.
commit_new() { # path -> commits a new file at it on top of base
    mkdir -p "$REPO/$(dirname "$1")"
    echo x >"$REPO/$1"
    (cd "$REPO" && git add -A >/dev/null 2>&1 && git commit -qm "add $1" >/dev/null 2>&1)
}
reset_repo
commit_new plugins/logos/hooks/session-start.sh
changed "[condition] a COMMITTED plugin-only change under plugins/ runs the leg" 0
echo more >>"$REPO/plugins/logos/hooks/session-start.sh"
(cd "$REPO" && git add -A >/dev/null 2>&1 && git commit -qm base2 >/dev/null 2>&1 && git tag -f base2 >/dev/null 2>&1)
echo more >>"$REPO/plugins/logos/hooks/session-start.sh"
GB=base2
changed "[condition] an UNCOMMITTED plugin-only change under plugins/ runs the leg" 0
GB=base
reset_repo
commit_new .claude-plugin/marketplace.json
changed "[condition] a change to .claude-plugin/ (the marketplace) runs the leg" 0
reset_repo
mkdir -p "$REPO/docs"
echo d >"$REPO/docs/d.md"
commit_new plugins/logos/.claude-plugin/plugin.json
changed "[condition] plugins/ among other changed paths, not first, runs the leg" 0
for near in plugins-old/x.sh src/plugins/x.sh .claude-plugins/x.json .claude/x.json docs/plugins.md; do
    reset_repo
    commit_new "$near"
    changed "[condition] $near is not the plugin tree (near miss)" 1
done

# --- 3. the leg's verdict -----------------------------------------------------
#
# leg <fixture-root> runs gate_script_suites over a root holding fixture suites.
# `record` prints what the gate would; the evidence lands in <root>/evidence.

mkroot() { # name -> a fixture root with scripts/tests/
    local r="$WORK/leg-$1"
    mkdir -p "$r/scripts/tests" "$r/evidence"
    echo "$r"
}
suite() { # root name exit-code body...
    local r="$1" n="$2" rc="$3"
    shift 3
    printf '#!/usr/bin/env bash\n%s\nexit %s\n' "$*" "$rc" >"$r/scripts/tests/$n.sh"
}
leg() { # root -> prints the leg's stdout; exit status is the subshell's
    local r="$1"
    (
        set -uo pipefail
        ROOT="$r" EVID="$r/evidence" TIER=full TREE_ID=t HEAD_SHA=h RUNNER=r WORKTREE=w
        SKIPS=()
        eval "$FUNCS"
        gate_done() { [ "${LEG_CACHED:-no}" = yes ]; }
        record() { printf '  %s %s %s\n' "$1" "$2" "${3-}"; }
        gate_script_suites
    )
}
# The expressions below are this file's own literals over the evidence it just
# wrote, never external input: eval is the point of the probe.
field() { # evidence-json python-expression-over-d
    python3 -I -c 'import json,sys
d = json.load(open(sys.argv[1]))
print(eval(sys.argv[2]))' "$1" "$2"
}
unit() { # evidence-json unit field
    field "$1" "next((u['$3'] for u in d['units'] if u['unit'] == '$2'), 'ABSENT')"
}

R="$(mkroot green)"
suite "$R" test-alpha 0 'echo "5 passed, 0 failed (of 5)"'
suite "$R" test-beta 0 'echo "passed 3, failed 0"'
OUT="$(leg "$R")"
J="$R/evidence/script-suites.json"
expect "[leg] two green suites: the gate verdict is pass" "$(field "$J" "d['verdict']")" "pass"
expect "[leg] one unit per suite, in name order" "$(field "$J" "[u['unit'] for u in d['units']]")" "['test-alpha', 'test-beta']"
expect "[leg] a green suite's counters (1 of 1; its own passed/failed)" \
    "$(field "$J" "[(u['expected'],u['started'],u['finished'],u['passed'],u['failed'],u['exit_code'],u['verdict']) for u in d['units']]")" \
    "[(1, 1, 1, 5, 0, 0, 'pass'), (1, 1, 1, 3, 0, 0, 'pass')]"
expect "[leg] the unit's argv names the suite it ran" "$(unit "$J" test-beta argv)" "env -u GATE_FORCE -u GATE_BASE bash scripts/tests/test-beta.sh"
expect "[leg] the gate-level argv names the roster glob" "$(field "$J" "d['argv']")" "bash scripts/tests/test-*.sh"
expect "[leg] the gate-level exit code and truncation" "$(field "$J" "(d['exit_code'], d['truncation'])")" "(0, 'none')"
expect "[leg] every unit's floor and truncation" "$(field "$J" "[(u['floor'],u['truncation']) for u in d['units']]")" "[(1, 'none'), (1, 'none')]"
case "$OUT" in
    *"script-suites pass (2 suite(s); 8 passed, 0 failed)"*) ok "[leg] the printed line names the suites run and the checks counted" ;;
    *) bad "[leg] printed: $OUT" ;;
esac
case "$OUT" in
    *"test-alpha"*"test-beta"*) ok "[leg] each suite is printed as it runs" ;;
    *) bad "[leg] suites not printed: $OUT" ;;
esac

R="$(mkroot red)"
suite "$R" test-ok 0 'echo "4 passed, 0 failed"'
suite "$R" test-red 1 'echo "2 passed, 1 failed"'
OUT="$(leg "$R")"
J="$R/evidence/script-suites.json"
expect "[leg] one red suite fails the gate" "$(field "$J" "(d['verdict'], d['exit_code'])")" "('fail', 1)"
expect "[leg] the red suite's unit records its failure and exit code" \
    "$(field "$J" "[(u['unit'],u['failed'],u['exit_code'],u['verdict']) for u in d['units'] if u['unit']=='test-red']")" "[('test-red', 1, 1, 'fail')]"
expect "[leg] a plainly red suite ran to an exit: finished, not truncated, not 'killed'" \
    "$(field "$J" "[(u['started'],u['finished'],u['truncation']) for u in d['units'] if u['unit']=='test-red']")" "[(1, 1, 'none')]"
expect "[leg] the green suite beside it stays pass" "$(unit "$J" test-ok verdict)" "pass"
case "$OUT" in *"script-suites fail"*) ok "[leg] the printed verdict is fail" ;; *) bad "[leg] printed: $OUT" ;; esac
case "$OUT" in
    *"(2 suite(s); 6 passed, 1 failed)"*) ok "[leg] the printed totals count the failure" ;;
    *) bad "[leg] printed totals: $OUT" ;;
esac
case "$OUT" in
    *"test-red"*"fail  2 passed, 1 failed"*"(exit 1; $R/evidence/script-suites--test-red.log)"*) ok "[leg] a failing suite's line names its exit code and its log" ;;
    *) bad "[leg] failing line: $OUT" ;;
esac

R="$(mkroot nocount)"
suite "$R" test-silent 0 'echo "it ran, honest"'
OUT="$(leg "$R")"
J="$R/evidence/script-suites.json"
expect "[leg] a suite that exits 0 having counted nothing proved nothing: fail" "$(field "$J" "d['verdict']")" "fail"
expect "[leg] ... and its unit records one failure" "$(unit "$J" test-silent failed)" "1"
case "$OUT" in *"counted no passing check"*) ok "[leg] ... and says why" ;; *) bad "[leg] printed: $OUT" ;; esac

R="$(mkroot zerocount)"
suite "$R" test-zero 0 'echo "0 passed, 0 failed"'
leg "$R" >/dev/null
expect "[leg] '0 passed, 0 failed' is the same as no summary: fail" "$(field "$R/evidence/script-suites.json" "d['verdict']")" "fail"

R="$(mkroot exitonly)"
suite "$R" test-exit 3 'echo "4 passed, 0 failed"'
leg "$R" >/dev/null
J="$R/evidence/script-suites.json"
expect "[leg] a non-zero exit with no counted failure still fails" "$(field "$J" "d['verdict']")" "fail"
expect "[leg] ... and is recorded as one failed check, with the exit code" \
    "$(field "$J" "[(u['failed'],u['exit_code']) for u in d['units']]")" "[(1, 3)]"
expect "[leg] ... having finished (an exit of 3 is not a kill)" \
    "$(field "$J" "[(u['finished'],u['truncation']) for u in d['units']]")" "[(1, 'none')]"

R="$(mkroot killed)"
suite "$R" test-killed 0 'echo "3 passed, 0 failed"; kill -9 $$'
leg "$R" >/dev/null
J="$R/evidence/script-suites.json"
expect "[leg] a suite killed by a signal fails" "$(field "$J" "d['verdict']")" "fail"
expect "[leg] ... started but did not finish (the OOM-kill signature)" \
    "$(field "$J" "[(u['started'],u['finished'],u['truncation']) for u in d['units']]")" "[(1, 0, 'killed')]"

R="$(mkroot empty)"
OUT="$(leg "$R")"
J="$R/evidence/script-suites.json"
expect "[leg] a glob matching no suite fails the leg" "$(field "$J" "(d['verdict'], d['exit_code'])")" "('fail', 1)"
expect "[leg] ... as a unit that started nothing" "$(field "$J" "[(u['started'],u['truncation']) for u in d['units']]")" "[(0, 'no_binaries')]"
expect "[leg] ... recorded as a failed, unfinished unit with a non-zero exit" \
    "$(field "$J" "[(u['unit'],u['finished'],u['failed'],u['exit_code'],u['verdict']) for u in d['units']]")" "[('(none)', 0, 0, 1, 'fail')]"
case "$OUT" in *"no scripts/tests/test-*.sh suite"*) ok "[leg] ... and says so" ;; *) bad "[leg] printed: $OUT" ;; esac

# A suite is run from the root with the gate's resume/base knobs unset: it builds
# its own fixtures and must not inherit a base it did not choose.
R="$(mkroot env)"
suite "$R" test-env 0 'echo "env: force=${GATE_FORCE-unset} base=${GATE_BASE-unset} cwd=$(pwd -P)"; echo "1 passed, 0 failed"'
(GATE_FORCE=yes GATE_BASE=somewhere leg "$R" >/dev/null)
LOGF="$R/evidence/script-suites--test-env.log"
case "$(cat "$LOGF" 2>/dev/null)" in
    *"force=unset base=unset cwd=$(cd "$R" && pwd -P)"*) ok "[leg] a suite runs from the root with GATE_FORCE and GATE_BASE unset" ;;
    *) bad "[leg] suite environment: $(cat "$LOGF" 2>/dev/null)" ;;
esac

# A suite's stderr is captured, its stdin is closed, the aggregate log holds every
# suite's output, and a log left by a suite that no longer exists is not folded in.
R="$(mkroot logs)"
suite "$R" test-noisy 0 'echo "to-stdout"; echo "to-stderr" >&2; echo "stdin=[$(cat)]"; echo "1 passed, 0 failed"'
printf 'stale\n' >"$R/evidence/script-suites--test-gone.log"
(leg "$R" <<<"leaked-from-the-gate" >/dev/null)
AGG="$(cat "$R/evidence/script-suites.log" 2>/dev/null)"
case "$AGG" in *"to-stderr"*) ok "[leg] a suite's stderr is in its log" ;; *) bad "[leg] stderr missing: $AGG" ;; esac
case "$AGG" in *"stdin=[]"*) ok "[leg] a suite's stdin is closed, not the gate's" ;; *) bad "[leg] stdin leaked: $AGG" ;; esac
case "$AGG" in *"to-stdout"*) ok "[leg] the aggregate log holds the suites' output" ;; *) bad "[leg] aggregate log: $AGG" ;; esac
case "$AGG" in *stale*) bad "[leg] a stale per-suite log was folded in: $AGG" ;; *) ok "[leg] a stale per-suite log of a removed suite is cleared" ;; esac

# Resuming: a leg that already passed for this tree is not re-run.
R="$(mkroot cached)"
suite "$R" test-never 1 'echo ran >"$PWD/ran-marker"'
OUT="$(LEG_CACHED=yes leg "$R")"
case "$OUT" in *"script-suites pass (cached, same tree)"*) ok "[leg] a leg already passed for this tree is reported cached" ;; *) bad "[leg] cached: $OUT" ;; esac
expect "[leg] ... and runs no suite" "$([ -e "$R/ran-marker" ] && echo ran || echo not-run)" "not-run"

# --- 4. the tier block --------------------------------------------------------
#
# The real `case "$TIER"` block of gate.sh, run with every gate replaced by a
# recorder. Whether the leg ran is read from the recorder, not from the output.

# From the `case "$TIER" in` line above the (unique) `    fast)` arm to its `esac`.
BLOCK="$(awk '
    { line[NR] = $0 }
    /^    fast\)$/ && !start { start = NR - 1 }
    start && /^esac$/ && !stop { stop = NR }
    END { if (start && stop) for (i = start; i <= stop; i++) print line[i] }
' "$GATE")"
if [ -z "$BLOCK" ]; then
    echo "FAIL  could not extract the tier block from $GATE"
    exit 1
fi

tiers() { # tier -> the gates the block ran, space-separated, then any "not run" note
    local t="$1" calls="$WORK/tier-calls"
    : >"$calls"
    (
        cd "$REPO" || exit 1
        eval "$FUNCS"
        TIER="$t" ROOT="$REPO" EVID="$WORK" GATES_RUN="" GATES_FAILED="" TREE_ID=t
        export GATE_BASE="${GB:-base}"
        for g in gate_clippy gate_tests gate_agents_marker gate_deny gate_arch gate_public_terms \
            gate_ui gate_ui_e2e gate_script_suites; do
            eval "$g() { echo $g >>\"$calls\"; }"
        done
        touched_packages() { :; }
        reset_units() { :; }
        write_evidence() { :; }
        record() { :; }
        compute_tree_id() { echo t; }
        pkg_names() { echo p; }
        eval "$BLOCK"
    ) >"$WORK/tier-notes" 2>&1
    tr '\n' ' ' <"$calls"
}

reset_repo
GB=base
expect "[tiers] full runs the leg with scripts/ unchanged" \
    "$(tiers full | tr ' ' '\n' | grep -c '^gate_script_suites$')" "1"
echo more >>"$REPO/scripts/a.sh"
(cd "$REPO" && git commit -qam "touch scripts" >/dev/null 2>&1)
expect "[tiers] full runs the leg with scripts/ changed" \
    "$(tiers full | tr ' ' '\n' | grep -c '^gate_script_suites$')" "1"
expect "[tiers] fast runs the leg when scripts/ changed" \
    "$(tiers fast | tr ' ' '\n' | grep -c '^gate_script_suites$')" "1"
reset_repo
echo more >>"$REPO/src/b.rs"
(cd "$REPO" && git commit -qam "touch src" >/dev/null 2>&1)
expect "[tiers] fast does not run the leg when scripts/ is unchanged" \
    "$(tiers fast | tr ' ' '\n' | grep -c '^gate_script_suites$')" "0"
case "$(cat "$WORK/tier-notes")" in
    *"script-suites: not run — scripts/, plugins/ and .claude-plugin/ are unchanged against the base base"*) ok "[tiers] ... and says it did not run, naming the base" ;;
    *) bad "[tiers] fast skipped the leg silently: $(cat "$WORK/tier-notes")" ;;
esac
expect "[tiers] fast still runs its other legs when it skips the suites" \
    "$(tiers fast | tr ' ' '\n' | grep -c -E '^(gate_clippy|gate_arch|gate_public_terms)$')" "3"
reset_repo
commit_new plugins/logos/hooks/session-start.sh
expect "[tiers] fast runs the leg when only the plugin tree changed" \
    "$(tiers fast | tr ' ' '\n' | grep -c '^gate_script_suites$')" "1"

echo
echo "$PASS passed, $FAIL failed (of $((PASS + FAIL)))"
[ "$FAIL" -eq 0 ] || exit 1
exit 0
