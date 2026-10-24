#!/usr/bin/env bash
#
# test-plugin-review-skill.sh — prove the logos-review skill of the logos Claude
# Code plugin (plugins/logos/skills/logos-review/, S-681, FR-IN-15) judges each
# delta entry by the rules a script can state, and proves a fix by re-measurement.
#
# The skill's judgement is the model's and is not tested here. What is tested is
# every deterministic step it stands on, each script against a fixture, each
# stating its denominator:
#
#   skills/logos-review/scripts/merge-base.sh      the base and the merge-base
#   skills/logos-review/scripts/review-delta.sh    `logos review --json`, kept
#   skills/logos-review/scripts/coverage-lookup.sh test reach of each symbol
#   skills/logos-review/scripts/trust-gate.sh      the proposed verdict
#   skills/logos-review/scripts/edit-scope.sh      only the fix files were edited
#   skills/logos-review/scripts/remeasure.sh       a fix proven by the delta
#   skills/logos-review/scripts/check-report.sh    the evidence rule on the report
#   skills/logos-review/scripts/debt-list.sh       debt mode's list
#
# `logos` is a stub that answers the documented JSON of `review`, `affected`,
# `impact`, `improvements`, `languages` and `status` from fixture files, so no
# binary, index or network is involved. The stub's shapes are the ones in
# docs/howto/commands.md and logos-core/src/models/{quality,navigation}.rs.
#
# Layers:
#   1. merge-base.sh: default and stated base, no base, no shared history, not a
#      repository, the identical-branch short-cut;
#   2. review-delta.sh: measured, not measured, old binary, missing prerequisites;
#   3. the fixture of the acceptance criteria — a trusted covered offender, an
#      uncovered one and a `same-file-only` one: coverage, then the verdicts
#      fix / test-first / untrusted, the near misses of the trust label, and the
#      other gate rules;
#   4. the whole loop on that fixture: only the fix symbol's file is edited, the
#      re-run reads `resolved`, the report cites it and the passing tests, and
#      carries no aggregate-signal claim;
#   5. re-measurement: every way a fix is not proven;
#   6. the report check: each rule, each by a report one line from valid;
#   7. a clean branch: no entries, no edit;
#   8. debt mode, and the skill's files (frontmatter, the scripts it names). Debt
#      mode names a unit by its string `node_id` (a group by its own reason, an
#      integer or null id by the counted fallback to name, file and dimension) and
#      re-measures by that id: a renamed near miss and a second same-named symbol
#      in the file are never the unit. Its string-id layer runs under each bash.
# Layers 1, 4, 7 and the string-id half of 8 run under each distinct bash on this machine (/bin/bash is
# 3.2 on macOS).
#
# Usage: bash scripts/tests/test-plugin-review-skill.sh
# Exit:  0 all cases behaved, 1 a case did not

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PLUGIN="$HERE/plugins/logos"
SKILL="$PLUGIN/skills/logos-review"
SCR="$SKILL/scripts"

WORK="${TMPDIR:-/tmp}/logos-plugin-review-skill-test.$$"
trap 'rm -rf "$WORK"' EXIT INT TERM
mkdir -p "$WORK" || exit 1
WORK="$(cd "$WORK" && pwd -P)"
export GIT_CEILING_DIRECTORIES="$WORK"

PASS=0
FAIL=0
ok() { echo "  ok    $1"; PASS=$((PASS + 1)); }
bad() { echo "  FAIL  $1"; FAIL=$((FAIL + 1)); }
expect() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1: got '$2', wanted '$3'"; fi; }
contains() { case "$2" in *"$3"*) ok "$1" ;; *) bad "$1: '$3' not in '$2'" ;; esac; }
lacks() { case "$2" in *"$3"*) bad "$1: '$3' found in '$2'" ;; *) ok "$1" ;; esac; }

for f in merge-base review-delta coverage-lookup trust-gate edit-scope remeasure check-report debt-list; do
    [ -f "$SCR/$f.sh" ] || { echo "FAIL  missing $SCR/$f.sh"; echo "0 passed, 1 failed (of 1)"; exit 1; }
done
[ -f "$SCR/lib/delta.py" ] && [ -f "$SCR/lib/common.sh" ] ||
    { echo "FAIL  missing scripts/lib"; echo "0 passed, 1 failed (of 1)"; exit 1; }

BASHES=()
for b in /bin/bash "$(command -v bash)"; do
    [ -x "$b" ] || continue
    r="$(cd "$(dirname "$b")" && pwd -P)/$(basename "$b")"
    dup=no
    for s in "${BASHES[@]+"${BASHES[@]}"}"; do [ "$s" = "$r" ] && dup=yes; done
    [ "$dup" = yes ] || BASHES+=("$r")
done

# --- fixture: the stub logos --------------------------------------------------------

FX="$WORK/fx"
BIN="$WORK/bin"
CALLS="$WORK/calls"
mkdir -p "$FX" "$BIN"

# Builtins only, so the same stub also runs under a PATH that holds nothing else.
cat >"$BIN/logos" <<'STUB'
#!/bin/bash
printf '%s\n' "$*" >>"$STUB_CALLS"
emit() { local l; while IFS= read -r l || [ -n "$l" ]; do printf '%s\n' "$l"; done <"$1"; }
sub="${1:-}"
[ $# -gt 0 ] && shift
case "$sub" in
    --version) echo "logos ${STUB_VERSION:-99.0.0}" ;;
    review)
        if [ -n "${STUB_OLD:-}" ]; then
            echo "error: unrecognized subcommand 'review'" >&2
            exit 2
        fi
        echo "merge-base before-state read from the cache" >&2
        emit "$STUB_DIR/${STUB_REVIEW:-review}.json"
        exit "${STUB_REVIEW_RC:-0}"
        ;;
    sync) exit "${STUB_SYNC_RC:-0}" ;;
    affected)
        f="${*: -1}"
        k="${f//[^A-Za-z0-9]/_}"
        if [ -f "$STUB_DIR/affected.$k.json" ]; then
            emit "$STUB_DIR/affected.$k.json"
        else
            printf '{"changed":["%s"],"tests_only":true,"affected":[],"unknown":["%s"]}\n' "$f" "$f"
        fi
        ;;
    impact)
        k="${1//[^A-Za-z0-9]/_}"
        if [ -f "$STUB_DIR/impact.$k.json" ]; then
            emit "$STUB_DIR/impact.$k.json"
        else
            printf '{"query":"%s","resolved":null,"upstream":[],"downstream":[]}\n' "$1"
        fi
        ;;
    improvements | languages | status) emit "$STUB_DIR/$sub.json" ;;
    *)
        echo "error: unrecognized subcommand '$sub'" >&2
        exit 2
        ;;
esac
STUB
chmod +x "$BIN/logos"

# The three offenders of the acceptance criteria. All three are new on the branch
# but one: covered_fn was already over its threshold at the merge-base (5) and the
# branch made it worse (6), so a fix reads `resolved` against the merge-base.
offender() { # status node name file line before after trust
    printf '{"dimension":"nesting","status":"%s","node_id":%s,"name":"%s","file":"%s","line":%s,"measure":"nesting_depth","before":%s,"after":%s,"threshold":4,"detail":"nesting depth %s","trust":"%s"}' \
        "$1" "$2" "$3" "$4" "$5" "$6" "$7" "${7/null/0}" "$8"
}
COVERED_ID='"src/`covered.rs`/covered_fn()."'
UNCOVERED_ID='"src/`uncovered.rs`/bare_fn()."'
THIN_ID='"src/`thin.py`/thin_fn()."'
COUNTS='{"new":0,"worsened":0,"improved":0,"resolved":0,"unchanged":0}'
review_doc() { # offenders-json-array measured offender-counts
    printf '{"base":"main","base_origin":"default-branch","head":"h1","merge_base":"m1","measured":%s,"not_measured_because":%s,"before_state":"shadow-index","offenders":%s,"offender_counts":%s,"violations":[],"violation_counts":%s,"introduced_clone_groups":[],"stale_files":[],"sections":{},"notes":[]}\n' \
        "${2:-true}" "$([ "${2:-true}" = true ] && echo null || echo '"base does not resolve"')" "$1" "${3:-$COUNTS}" "$COUNTS"
}
FULL="[$(offender worsened "$COVERED_ID" covered_fn src/covered.rs 10 5 6 resolved),$(offender new "$UNCOVERED_ID" bare_fn src/uncovered.rs 20 null 6 resolved),$(offender new "$THIN_ID" thin_fn src/thin.py 5 null 5 same-file-only)]"
review_doc "$FULL" true '{"new":2,"worsened":1,"improved":0,"resolved":0,"unchanged":7}' >"$FX/review.json"
review_doc "[]" true '{"new":0,"worsened":0,"improved":0,"resolved":0,"unchanged":10}' >"$FX/review-clean.json"
review_doc "[]" false >"$FX/review-unmeasured.json"
# After the fix: covered_fn is gone from the branch's offenders — `resolved`.
AFTER_RESOLVED="[$(offender resolved "$COVERED_ID" covered_fn src/covered.rs 10 5 null resolved),$(offender new "$UNCOVERED_ID" bare_fn src/uncovered.rs 20 null 6 resolved),$(offender new "$THIN_ID" thin_fn src/thin.py 5 null 5 same-file-only)]"
review_doc "$AFTER_RESOLVED" >"$FX/review-resolved.json"
review_doc "[$(offender improved "$COVERED_ID" covered_fn src/covered.rs 10 5 4 resolved)]" >"$FX/review-improved.json"
review_doc "[$(offender worsened "$COVERED_ID" covered_fn src/covered.rs 10 5 6 resolved)]" >"$FX/review-still.json"
review_doc "[$(offender resolved "$COVERED_ID" covered_fn src/covered.rs 10 5 null stale)]" >"$FX/review-stale.json"
review_doc "[]" >"$FX/review-gone.json"

# Test reach: covered.rs and thin.py have a test file depending on them; uncovered.rs
# has none; covered_fn is called from that test file.
affected() { printf '{"changed":["%s"],"tests_only":true,"affected":[{"file":"%s","distance":1,"is_test":true}],"unknown":[]}\n' "$1" "$2"; }
affected src/covered.rs tests/covered_test.rs >"$FX/affected.src_covered_rs.json"
affected src/thin.py tests/test_thin.py >"$FX/affected.src_thin_py.json"
printf '{"changed":["src/uncovered.rs"],"tests_only":true,"affected":[],"unknown":[]}\n' >"$FX/affected.src_uncovered_rs.json"
impact() { # symbol file upstream-file
    printf '{"query":"%s","resolved":{"symbol":"%s","name":"x","kind":"function","file":"%s","line":1},"upstream":[{"symbol":"t","name":"t","kind":"function","file":"%s","line":3,"distance":1}],"downstream":[]}\n' "$1" "$1" "$2" "$3"
}
k() { printf '%s' "${1//[^A-Za-z0-9]/_}"; }
impact 'src/`covered.rs`/covered_fn().' src/covered.rs tests/covered_test.rs >"$FX/impact.$(k 'src/`covered.rs`/covered_fn().').json"
impact 'src/`thin.py`/thin_fn().' src/thin.py tests/test_thin.py >"$FX/impact.$(k 'src/`thin.py`/thin_fn().').json"

# A scratch git repository with the three source files and the test file.
REPO="$WORK/repo"
mkdir -p "$REPO/src" "$REPO/tests" "$REPO/.logos"
git_() { git -C "$REPO" -c user.email=t@t -c user.name=t -c commit.gpgsign=false -c maintenance.auto=false -c gc.auto=0 "$@"; }
git_ init -q -b main . >/dev/null 2>&1 || { git -C "$REPO" init -q . && git_ checkout -q -b main; }
printf 'fn covered_fn() {}\n' >"$REPO/src/covered.rs"
printf 'fn bare_fn() {}\n' >"$REPO/src/uncovered.rs"
printf 'def thin_fn(): pass\n' >"$REPO/src/thin.py"
printf '#[test] fn t() {}\n' >"$REPO/tests/covered_test.rs"
printf '.logos/\n' >"$REPO/.gitignore"
git_ add -A >/dev/null && git_ commit -q -m base
BASE_SHA="$(git_ rev-parse HEAD)"

# env <extra VAR=VALUE>... -- command: the stub first on PATH, the repo as the project.
export STUB_CALLS="$CALLS" STUB_DIR="$FX"
run() { # bash script args... -> sets OUT (stdout), ERR (stderr), RC
    local b="$1" s="$2"
    shift 2
    RC=0
    OUT="$(cd "$REPO" && PATH="$BIN:$PATH" LOGOS_PROJECT_DIR="$REPO" "$b" "$SCR/$s" "$@" 2>"$WORK/err")" || RC=$?
    ERR="$(cat "$WORK/err")"
}
last_line() { printf '%s\n' "$1" | tail -1; }
# jget FILE EXPR — EXPR over the parsed document `d`. The eval is over an expression
# written in this file, never over fixture or script output.
jget() { python3 -I -c 'import json,sys; d=json.load(open(sys.argv[1])); print(eval(sys.argv[2]))' "$1" "$2"; }
verdicts() { # triage.json -> name:proposed,... in entry order
    python3 -I - "$1" <<'PYEOF'
import json, sys
d = json.load(open(sys.argv[1]))
print(",".join(e["name"] + ":" + e["proposed"] for e in d["entries"]))
PYEOF
}

B="${BASHES[0]}"

# --- 1. merge-base.sh -----------------------------------------------------------------

echo "== merge-base.sh"
for B in "${BASHES[@]}"; do
    T="[$B]"
    run "$B" merge-base.sh
    expect "$T default base: exit 0" "$RC" 0
    contains "$T default base is main, origin 'default-branch'" "$OUT" "base=main"
    contains "$T ... base_origin" "$OUT" "base_origin=default-branch"
    contains "$T ... merge_base is the commit" "$OUT" "merge_base=$BASE_SHA"
    contains "$T on the base itself: identical=yes" "$OUT" "identical=yes"
    contains "$T ... denominator names the refs tried" "$(last_line "$ERR")" "denominator: "
    contains "$T ... 1 candidate tried (no origin/HEAD, main resolves first of main master)" "$ERR" "1 resolved"

    run "$B" merge-base.sh --base "$BASE_SHA"
    contains "$T a stated base: base_origin=stated" "$OUT" "base_origin=stated"
    run "$B" merge-base.sh --base no-such-ref
    expect "$T an unknown base: exit 1" "$RC" 1
    contains "$T ... says unresolved" "$OUT" "unresolved"
    contains "$T ... denominator reads 0 resolved" "$ERR" "0 resolved"
    run "$B" merge-base.sh --bogus
    expect "$T an unknown argument: exit 2" "$RC" 2
done

# A branch with a commit, then a dirty path, then only .logos/ noise.
git_ checkout -q -b feature
printf 'fn covered_fn() { if a { if b {} } }\n' >"$REPO/src/covered.rs"
git_ commit -q -am "branch change"
run "$B" merge-base.sh
contains "branch: one commit past the merge-base" "$OUT" "branch_commits=1"
contains "branch: identical=no" "$OUT" "identical=no"
contains "branch: the merge-base is still the base commit" "$OUT" "merge_base=$BASE_SHA"
git_ checkout -q main
printf 'x\n' >"$REPO/.logos/noise"
run "$B" merge-base.sh
contains "a file under .logos/ is not a dirty path" "$OUT" "dirty_paths=0"
contains "... so the branch is still identical" "$OUT" "identical=yes"
printf 'y\n' >"$REPO/src/new.rs"
run "$B" merge-base.sh
contains "an untracked source file is a dirty path" "$OUT" "dirty_paths=1"
contains "... so the branch differs" "$OUT" "identical=no"
rm -f "$REPO/src/new.rs"

ORPHAN="$WORK/orphan"
git -C "$REPO" worktree add -q --detach "$ORPHAN" HEAD >/dev/null 2>&1
git -C "$ORPHAN" checkout -q --orphan unrelated >/dev/null 2>&1
git -C "$ORPHAN" -c user.email=t@t -c user.name=t -c commit.gpgsign=false commit -q --allow-empty -m other >/dev/null 2>&1
RC=0
OUT="$(PATH="$BIN:$PATH" "$B" "$SCR/merge-base.sh" --project "$ORPHAN" --base main 2>"$WORK/err")" || RC=$?
expect "no shared history with the base: exit 1" "$RC" 1
contains "... says so" "$OUT" "share no history"
git -C "$REPO" worktree remove --force "$ORPHAN" >/dev/null 2>&1

mkdir -p "$WORK/plain"
RC=0
OUT="$("$B" "$SCR/merge-base.sh" --project "$WORK/plain" 2>"$WORK/err")" || RC=$?
expect "not a git repository: exit 3" "$RC" 3

# --- 2. review-delta.sh -----------------------------------------------------------------

echo "== review-delta.sh"
T="[$B]"
run "$B" review-delta.sh
expect "no --out: exit 2" "$RC" 2
run "$B" review-delta.sh --out "$WORK/d.json"
expect "a measured delta: exit 0" "$RC" 0
contains "... the summary names the base, merge-base and before-state" "$OUT" "base main merge-base m1 before-state shadow-index"
contains "... 3 entries to judge" "$OUT" "3 entries to judge"
contains "... the denominator counts the unchanged entries it did not list" "$ERR" "3 listed of 10 offender and violation entries counted (7 unchanged, not listed)"
contains "... logos's own cache note is passed through" "$ERR" "before-state read from the cache"
expect "... the JSON was kept" "$(jget "$WORK/d.json" 'len(d["offenders"])')" 3
expect "... no temp file left behind" "$(find "$WORK" -maxdepth 1 -name 'd.json.*' | grep -c .)" 0
run "$B" review-delta.sh --out "$WORK/d2.json" --base release/1
contains "--base is passed to logos review" "$(grep '^review' "$CALLS" | tail -1)" "--base release/1"

STUB_REVIEW=review-unmeasured STUB_REVIEW_RC=1 run "$B" review-delta.sh --out "$WORK/u.json"
expect "a review that measured nothing: exit 1" "$RC" 1
contains "... the summary says why" "$OUT" "not measured: base does not resolve"
contains "... the denominator is 0 entries" "$ERR" "0 entries classified"
STUB_VERSION=1.19.0 run "$B" review-delta.sh --out "$WORK/o.json"
expect "a logos older than 1.20.0: exit 3" "$RC" 3
contains "... names the version it has and the one it needs" "$ERR" "logos 1.19.0 has no \`review\` command"
contains "... and 1.20.0" "$ERR" "1.20.0 or newer"
expect "... and leaves no output file" "$([ -e "$WORK/o.json" ] && echo kept || echo none)" none
STUB_VERSION=1.100.0 run "$B" review-delta.sh --out "$WORK/o2.json"
expect "near miss: 1.100.0 is newer than 1.20.0 (numeric, not lexical)" "$RC" 0
STUB_VERSION=garbage run "$B" review-delta.sh --out "$WORK/o3.json"
expect "a logos whose version cannot be read: exit 3" "$RC" 3
contains "... says so" "$ERR" "did not report a version"
STUB_OLD=1 run "$B" review-delta.sh --out "$WORK/o4.json"
expect "a review that fails outright (exit 2): exit 3, never 'not measured'" "$RC" 3
contains "... says it failed with no answer" "$ERR" "failed with exit 2 and no answer"
expect "... and leaves no output file" "$([ -e "$WORK/o4.json" ] && echo kept || echo none)" none
expect "... and no temp file" "$(find "$WORK" -maxdepth 1 -name 'o4.json.*' | grep -c .)" 0
STUB_REVIEW_RC=7 run "$B" review-delta.sh --out "$WORK/o5.json"
expect "a review that exits 7: exit 3" "$RC" 3
: >"$FX/review-empty.json"
STUB_REVIEW=review-empty run "$B" review-delta.sh --out "$WORK/o6.json"
expect "a review that prints nothing and exits 0: exit 3, no answer" "$RC" 3
expect "... and no output file is left for a later step to read" "$([ -e "$WORK/o6.json" ] && echo kept || echo none)" none
run "$B" review-delta.sh --out "$WORK/no-such-dir/d.json"
expect "an --out in a directory that does not exist: exit 3, not 'not measured'" "$RC" 3
(cd "$WORK" && PATH="$BIN:$PATH" LOGOS_PROJECT_DIR="$REPO" "$B" "$SCR/review-delta.sh" --out rel.json >/dev/null 2>&1)
expect "a relative --out lands beside the caller, not in the project" "$([ -s "$WORK/rel.json" ] && [ ! -e "$REPO/rel.json" ] && echo yes || echo no)" yes

RC=0
OUT="$(cd "$REPO" && PATH="$(dirname "$(command -v python3)"):/usr/bin:/bin" LOGOS_PROJECT_DIR="$REPO" "$B" "$SCR/review-delta.sh" --out "$WORK/n.json" 2>"$WORK/err")" || RC=$?
if command -v logos >/dev/null 2>&1 && [ "$(PATH="$(dirname "$(command -v python3)"):/usr/bin:/bin" command -v logos || true)" != "" ]; then
    ok "(a real logos is on the minimal PATH here: the no-logos case is skipped)"
else
    expect "no logos on PATH: exit 3" "$RC" 3
    contains "... says to install it" "$(cat "$WORK/err")" "no logos binary"
fi
mkdir -p "$WORK/unenrolled"
git -C "$WORK/unenrolled" init -q . >/dev/null 2>&1
RC=0
(cd "$WORK/unenrolled" && PATH="$BIN:$PATH" LOGOS_PROJECT_DIR="$WORK/unenrolled" "$B" "$SCR/review-delta.sh" --out "$WORK/e.json" >/dev/null 2>"$WORK/err") || RC=$?
expect "a repository with no .logos/: exit 3" "$RC" 3
contains "... says to run init" "$(cat "$WORK/err")" "logos init"
mkdir -p "$WORK/sys" "$WORK/nopy-bin"
ln -sf "$B" "$WORK/sys/bash"
cp "$BIN/logos" "$WORK/nopy-bin/logos"
RC=0
(cd "$REPO" && env -i HOME="$WORK" PATH="$WORK/nopy-bin:$WORK/sys" LOGOS_PROJECT_DIR="$REPO" STUB_CALLS="$CALLS" STUB_DIR="$FX" "$B" "$SCR/review-delta.sh" --out "$WORK/p.json" >/dev/null 2>"$WORK/err") || RC=$?
expect "no python3 on PATH: exit 3" "$RC" 3
contains "... names python3" "$(cat "$WORK/err")" "python3"

# --- 3. the fixture of the acceptance criteria -------------------------------------------

echo "== coverage-lookup.sh and trust-gate.sh"
run "$B" review-delta.sh --out "$WORK/delta.json"
run "$B" coverage-lookup.sh --delta "$WORK/delta.json" --out "$WORK/cov.json"
expect "coverage-lookup: exit 0" "$RC" 0
expect "the trusted covered offender is covered (a test file reaches the symbol)" \
    "$(jget "$WORK/cov.json" '[c["coverage"] for c in d["coverage"]][0]')" covered
expect "... citing the test file" "$(jget "$WORK/cov.json" 'd["coverage"][0]["tests"]')" "['tests/covered_test.rs']"
expect "the uncovered offender is uncovered" "$(jget "$WORK/cov.json" 'd["coverage"][1]["coverage"]')" uncovered
expect "the same-file-only offender is looked up too (its file has a test)" "$(jget "$WORK/cov.json" 'd["coverage"][2]["coverage"]')" covered
contains "the denominator: entries, with a symbol, calls, per-answer tally" "$(last_line "$ERR")" "denominator: 3 entries, 3 with a symbol"
contains "... the tally" "$(last_line "$ERR")" "covered 2, uncovered 1"
contains "... and the limit of test reach by file is stated" "$(last_line "$ERR")" "not seen"

# the near misses of `covered`
cp "$FX/impact.$(k 'src/`covered.rs`/covered_fn().').json" "$WORK/impact.keep"
impact 'src/`covered.rs`/covered_fn().' src/covered.rs src/other_caller.rs >"$FX/impact.$(k 'src/`covered.rs`/covered_fn().').json"
run "$B" coverage-lookup.sh --delta "$WORK/delta.json" --out "$WORK/cov2.json"
expect "near miss: tests depend on the file but a non-test file calls the symbol -> file-only" \
    "$(jget "$WORK/cov2.json" 'd["coverage"][0]["coverage"]')" file-only
impact 'src/`covered.rs`/covered_fn().' src/elsewhere.rs tests/covered_test.rs >"$FX/impact.$(k 'src/`covered.rs`/covered_fn().').json"
run "$B" coverage-lookup.sh --delta "$WORK/delta.json" --out "$WORK/cov3.json"
expect "near miss: the name resolves to another file -> unknown, never credited" \
    "$(jget "$WORK/cov3.json" 'd["coverage"][0]["coverage"]')" unknown
cp "$WORK/impact.keep" "$FX/impact.$(k 'src/`covered.rs`/covered_fn().').json"
mv "$FX/affected.src_covered_rs.json" "$WORK/affected.keep"
run "$B" coverage-lookup.sh --delta "$WORK/delta.json" --out "$WORK/cov4.json"
expect "a file the graph does not know -> unknown" "$(jget "$WORK/cov4.json" 'd["coverage"][0]["coverage"]')" unknown
mv "$WORK/affected.keep" "$FX/affected.src_covered_rs.json"
# The real binary's shape (found by UAT-IN-06, S-683): `logos impact` does not
# resolve a symbol id without its repository coordinate, only the bare name. The id
# is asked first, then the name it ends in.
mv "$FX/impact.$(k 'src/`covered.rs`/covered_fn().').json" "$WORK/impact.keep"
# impact_as QUERY SYMBOL FILE UPSTREAM-FILE [ALTERNATIVES-JSON]: the binary's answer
# to a bare-name query, which resolves to a full symbol (repository coordinate first).
impact_as() {
    printf '{"query":"%s","resolved":{"symbol":"%s","name":"x","kind":"function","file":"%s","line":1},"upstream":[{"symbol":"t","name":"t","kind":"function","file":"%s","line":3,"distance":1}],"downstream":[],"alternatives":%s}\n' "$1" "$2" "$3" "$4" "${5:-[]}"
}
OWN='logos . repo . src/`covered.rs`/covered_fn().'
SIB='logos . repo . src/`covered.rs`/Other#covered_fn().'
impact_as covered_fn "$OWN" src/covered.rs tests/covered_test.rs >"$FX/impact.covered_fn.json"
: >"$CALLS"
run "$B" coverage-lookup.sh --delta "$WORK/delta.json" --out "$WORK/cov5.json"
expect "an id impact does not resolve falls back to its bare name -> covered" \
    "$(jget "$WORK/cov5.json" 'd["coverage"][0]["coverage"]')" covered
expect "... asking the id first, then the name" \
    "$(grep -E '^impact (src/.covered.rs./covered_fn\(\)\.|covered_fn) ' "$CALLS" | cut -d' ' -f2 | tr '\n' ' ')" \
    'src/`covered.rs`/covered_fn(). covered_fn '
impact_as covered_fn 'logos . repo . src/`elsewhere.rs`/covered_fn().' src/elsewhere.rs tests/covered_test.rs >"$FX/impact.covered_fn.json"
run "$B" coverage-lookup.sh --delta "$WORK/delta.json" --out "$WORK/cov6.json"
expect "near miss: the bare name resolves in another file -> unknown, never credited" \
    "$(jget "$WORK/cov6.json" 'd["coverage"][0]["coverage"]')" unknown
# A same-named sibling in the SAME file (a method beside a function, A::new beside
# B::new) passes the file check; its tests are still not this symbol's.
impact_as covered_fn "$SIB" src/covered.rs tests/covered_test.rs >"$FX/impact.covered_fn.json"
run "$B" coverage-lookup.sh --delta "$WORK/delta.json" --out "$WORK/cov8.json"
expect "near miss: the bare name resolves to a same-file sibling -> unknown, never credited" \
    "$(jget "$WORK/cov8.json" 'd["coverage"][0]["coverage"]')" unknown
# The sibling resolves and the entry's own node is an alternative: it is asked
# again by its full symbol, and only its own answer counts (here: no test reaches it).
impact_as covered_fn "$SIB" src/covered.rs tests/covered_test.rs \
    "[{\"symbol\":\"$OWN\",\"name\":\"covered_fn\",\"kind\":\"function\",\"file\":\"src/covered.rs\",\"line\":10}]" >"$FX/impact.covered_fn.json"
impact_as "$OWN" "$OWN" src/covered.rs src/other_caller.rs >"$FX/impact.$(k "$OWN").json"
: >"$CALLS"
run "$B" coverage-lookup.sh --delta "$WORK/delta.json" --out "$WORK/cov9.json"
expect "the entry's own node among the alternatives is asked by its full symbol -> its answer (file-only)" \
    "$(jget "$WORK/cov9.json" 'd["coverage"][0]["coverage"]')" file-only
expect "... the third impact call names the full symbol" \
    "$(grep -c "^impact $OWN " "$CALLS")" 1
rm -f "$FX/impact.$(k "$OWN").json"
# bare_name over the id shapes logos writes. -B: no __pycache__ in the plugin tree.
BARE="$(python3 -I -B -c '
import sys
sys.path.insert(0, sys.argv[1])
from delta import bare_name
ids = ["src/`lib.rs`/deep().", "pkg/Foo#bar().", "src/`lib.rs`/new(1).", "src/`x.rs`/Foo#run(12).",
       "src/`m.rs`/Thing#", "web/`a.js`/$init().", "plain", "", "---"]
print(" ".join(str(bare_name(i)) for i in ids))' "$SCR/lib")"
expect "bare_name: free function, method, numbered overloads, type, \$ identifier, plain, empty, none" \
    "$BARE" "deep bar new run Thing \$init plain None None"
rm -f "$FX/impact.covered_fn.json"
run "$B" coverage-lookup.sh --delta "$WORK/delta.json" --out "$WORK/cov7.json"
expect "neither the id nor the name resolves -> unknown" \
    "$(jget "$WORK/cov7.json" 'd["coverage"][0]["coverage"]')" unknown
mv "$WORK/impact.keep" "$FX/impact.$(k 'src/`covered.rs`/covered_fn().').json"
run "$B" coverage-lookup.sh --delta "$WORK/delta.json"
expect "coverage-lookup without --out: exit 2" "$RC" 2

run "$B" trust-gate.sh --delta "$WORK/delta.json" --coverage "$WORK/cov.json" --out "$WORK/triage.json"
expect "trust-gate: exit 0" "$RC" 0
expect "VERDICTS: the covered offender is fix, the uncovered test-first, the same-file-only untrusted" \
    "$(verdicts "$WORK/triage.json")" "covered_fn:fix,bare_fn:test-first,thin_fn:untrusted"
expect "... only the fix is open to the model's judgement" \
    "$(jget "$WORK/triage.json" '",".join(str(e["final"]) for e in d["entries"])')" "False,True,True"
contains "... the table row marks the open verdict" "$OUT" "$(printf 'e1\tfix?\tworsened')"
contains "... the untrusted row says why" "$OUT" "trust label 'same-file-only': reported, never edited"
contains "... denominator" "$(last_line "$ERR")" "denominator: 3 entries gated: 1 fix, 1 test-first, 1 untrusted"

# the other rules and the near misses of the trust label
gate_of() { # coverage trust status node -> proposed verdict
    local d="$WORK/g.delta.json" c="$WORK/g.cov.json"
    review_doc "[$(offender "$3" "$4" some_fn src/some.rs 1 1 6 "$2")]" >"$d"
    printf '{"coverage":[{"id":"e1","key":"offender|nesting|nesting_depth|%s","coverage":"%s","tests":[],"note":""}]}\n' "$(printf '%s' "$4" | tr -d '"')" "$1" >"$c"
    bash "$SCR/trust-gate.sh" --delta "$d" --coverage "$c" --out "$WORK/g.out.json" >/dev/null 2>&1
    jget "$WORK/g.out.json" 'd["entries"][0]["proposed"]'
}
expect "stale is untrusted even when covered" "$(gate_of covered stale new "$COVERED_ID")" untrusted
expect "no-resolved-edges is untrusted" "$(gate_of covered no-resolved-edges new "$COVERED_ID")" untrusted
expect "near miss: a label that merely starts with 'resolved' is untrusted" "$(gate_of covered resolved-ish new "$COVERED_ID")" untrusted
expect "a missing trust label is untrusted" "$(gate_of covered '' new "$COVERED_ID")" untrusted
expect "untrusted outranks test-first" "$(gate_of uncovered same-file-only new "$UNCOVERED_ID")" untrusted
expect "a resolved entry needs no action" "$(gate_of covered resolved resolved "$COVERED_ID")" no-action-warranted
expect "an improved entry needs no action" "$(gate_of covered resolved improved "$COVERED_ID")" no-action-warranted
expect "an entry with no symbol is no-action-warranted" "$(gate_of covered resolved new null)" no-action-warranted
expect "file-only coverage is test-first" "$(gate_of file-only resolved worsened "$COVERED_ID")" test-first
expect "unknown coverage is test-first" "$(gate_of unknown resolved new "$COVERED_ID")" test-first
expect "a worsened covered trusted entry is a fix" "$(gate_of covered resolved worsened "$COVERED_ID")" fix
run "$B" trust-gate.sh --delta "$WORK/delta.json"
expect "trust-gate without --coverage: exit 2" "$RC" 2

# --- 4. the whole loop on the fixture -----------------------------------------------------

echo "== the loop: edit only the fix, re-measure, report"
for B in "${BASHES[@]}"; do
    T="[$B]"
    git_ checkout -q main 2>/dev/null
    git_ checkout -q -- . 2>/dev/null
    : >"$CALLS"
    STATE="$WORK/scope.state"
    run "$B" edit-scope.sh begin --state "$STATE"
    expect "$T edit-scope begin: exit 0" "$RC" 0
    contains "$T ... denominator" "$ERR" "0 dirty path(s) recorded at begin"

    # The skill edits the files of the entries proposed `fix` — and nothing else.
    FIXFILES="$(jget "$WORK/triage.json" '" ".join(e["file"] for e in d["entries"] if e["proposed"] == "fix")')"
    expect "$T the fix set is the one file of the trusted covered offender" "$FIXFILES" "src/covered.rs"
    printf 'fn covered_fn() { helper(); }\nfn helper() {}\n' >"$REPO/src/covered.rs"
    run "$B" edit-scope.sh check --state "$STATE" --allow src/covered.rs
    expect "$T only the fix symbol's file edited: scope holds (exit 0)" "$RC" 0
    contains "$T ... denominator" "$(last_line "$ERR")" "1 changed since begin, 1 allowed; 0 outside"
    run "$B" edit-scope.sh check --state "$STATE"
    expect "$T the same edit without an allowance is out of scope (exit 1)" "$RC" 1
    contains "$T ... the path is named" "$OUT" "outside the fix set: src/covered.rs"
    printf 'fn bare_fn() { changed(); }\n' >"$REPO/src/uncovered.rs"
    run "$B" edit-scope.sh check --state "$STATE" --allow src/covered.rs
    expect "$T editing the test-first symbol too breaks the scope (exit 1)" "$RC" 1
    contains "$T ... the uncovered file is named" "$OUT" "outside the fix set: src/uncovered.rs"
    printf 'def thin_fn(): return 1\n' >"$REPO/src/thin.py"
    run "$B" edit-scope.sh check --state "$STATE" --allow src/covered.rs
    contains "$T ... and so is the untrusted one" "$OUT" "outside the fix set: src/thin.py"
    git_ checkout -q -- src/uncovered.rs src/thin.py
    run "$B" edit-scope.sh check --state "$STATE" --allow src/covered.rs
    expect "$T reverting the stray edits restores the scope (exit 0)" "$RC" 0
    printf 'z\n' >"$REPO/src/created.rs"
    run "$B" edit-scope.sh check --state "$STATE" --allow src/covered.rs
    contains "$T a new untracked file is a change too" "$OUT" "outside the fix set: src/created.rs"
    rm -f "$REPO/src/created.rs"

    # Re-measure: the stub now answers the re-run, in which covered_fn is resolved.
    STUB_REVIEW=review-resolved run "$B" remeasure.sh --triage "$WORK/triage.json" --out "$WORK/after.json" --entry e1
    expect "$T remeasure after the fix: exit 0" "$RC" 0
    contains "$T ... the entry reads proven, with its status" "$OUT" "proven: e1 covered_fn (src/covered.rs:10): resolved,"
    contains "$T ... citing the values: 6 -> gone (merge-base 5)" "$OUT" "nesting_depth 6 -> gone (merge-base 5)"
    contains "$T ... the edited file was synced first" "$(grep '^sync' "$CALLS" | tail -1)" "sync -- src/covered.rs"
    contains "$T ... denominator" "$(last_line "$ERR")" "1 entry re-measured against 3 listed in the re-run"

    REPORT="$WORK/report.md"
    cat >"$REPORT" <<'EOF'
## logos-review

ENTRY | fix | src/covered.rs:10 covered_fn | trusted, covered, nesting 6 over 4
ENTRY | test-first | src/uncovered.rs:20 bare_fn | no test reach (uncovered)
ENTRY | untrusted | src/thin.py:5 thin_fn | trust label 'same-file-only'
FIXED | src/covered.rs:10 covered_fn | remeasured: resolved 6 -> gone | tests: cargo test | 12 passed, 0 failed (of 12)
EOF
    run "$B" check-report.sh --report "$REPORT"
    expect "$T the report cites the resolved re-measurement and the passing run: valid (exit 0)" "$RC" 0
    contains "$T ... denominator" "$(last_line "$ERR")" "3 ENTRY"
    git_ checkout -q -- .
done
expect "the stub's sync was called for the fix file only, never the other offenders" \
    "$(grep -c 'sync .*uncovered\|sync .*thin' "$CALLS")" 0

# --- 5. every way a fix is not proven -------------------------------------------------------

echo "== remeasure.sh"
rm_case() { # review-fixture-name -> output (stdout) of remeasure for e1, sets RC
    STUB_REVIEW="$1" run "$B" remeasure.sh --triage "$WORK/triage.json" --out "$WORK/after-$1.json" --entry e1
}
rm_case review-improved
expect "an improved entry is proven (exit 0)" "$RC" 0
contains "... as improved" "$OUT" "proven: e1 covered_fn (src/covered.rs:10): improved,"
rm_case review-still
expect "an entry still worsened is not proven (exit 1)" "$RC" 1
contains "... says still worsened" "$OUT" "not proven: e1 covered_fn (src/covered.rs:10): still worsened"
rm_case review-gone
expect "a worsened entry no longer listed is back at its merge-base value: not proven (exit 1)" "$RC" 1
contains "... says unchanged" "$OUT" "back at its merge-base value (unchanged)"
rm_case review-stale
expect "an entry the re-run labels stale is not proven (exit 1)" "$RC" 1
contains "... says it was not measured as edited" "$OUT" "labels it 'stale'"
STUB_REVIEW=review-unmeasured STUB_REVIEW_RC=1 run "$B" remeasure.sh --triage "$WORK/triage.json" --out "$WORK/after-u.json" --entry e1
expect "a re-run that measured nothing proves nothing (exit 1)" "$RC" 1
contains "... says so" "$OUT" "the re-run measured nothing"
contains "... denominator is 0 of 1" "$(last_line "$ERR")" "0 of 1 entries re-measured"
run "$B" remeasure.sh --triage "$WORK/triage.json" --out "$WORK/x.json" --entry e9
expect "an entry the triage never had: exit 2" "$RC" 2
run "$B" remeasure.sh --triage "$WORK/triage.json" --out "$WORK/x.json"
expect "no --entry: exit 2" "$RC" 2
STUB_SYNC_RC=1 STUB_REVIEW=review-resolved run "$B" remeasure.sh --triage "$WORK/triage.json" --out "$WORK/x.json" --entry e1
expect "a failed sync is an environment error, not a proof (exit 3)" "$RC" 3

# `cleared`: an entry that was new on the branch and was fixed has no entry to resolve.
rm_new() { STUB_REVIEW="$1" run "$B" remeasure.sh --triage "$WORK/triage.json" --out "$WORK/after-new.json" --entry e2; }
rm_new review-gone
expect "a NEW entry no longer listed is cleared, not resolved (exit 0)" "$RC" 0
contains "... the word is cleared" "$OUT" "cleared: e2 bare_fn"
contains "... and says the merge-base had nothing to resolve" "$OUT" "the merge-base had no entry to resolve"
lacks "... never 'resolved' for it" "$OUT" "proven"
printf '{"base":"main","head":"h","merge_base":"m","measured":true,"not_measured_because":null,"before_state":"shadow-index","offenders":[],"offender_counts":%s,"violations":[],"violation_counts":%s,"introduced_clone_groups":[],"stale_files":["src/uncovered.rs"],"sections":{},"notes":[]}\n' "$COUNTS" "$COUNTS" >"$FX/review-stalefile.json"
rm_new review-stalefile
expect "a new entry gone only because its file is stale is not cleared (exit 1)" "$RC" 1
contains "... says the file is still stale" "$OUT" "its file is still stale"

# --- 6. the report check ---------------------------------------------------------------------

echo "== check-report.sh"
GOOD="$WORK/good.md"
cat >"$GOOD" <<'EOF'
ENTRY | fix | src/covered.rs:10 covered_fn | trusted, covered
ENTRY | test-first | src/uncovered.rs:20 bare_fn | no test reach
ENTRY | no-action-warranted | src/old.rs:3 cold_fn | cold file: untouched for two years
ENTRY | untrusted | src/thin.py:5 thin_fn | trust label 'same-file-only'
FIXED | src/covered.rs:10 covered_fn | remeasured: resolved 6 -> gone | tests: cargo test | 12 passed, 0 failed (of 12)
EOF
report_case() { # label sed-expression expected-substring
    sed "$2" "$GOOD" >"$WORK/r.md"
    run "$B" check-report.sh --report "$WORK/r.md"
    expect "$1: exit 1" "$RC" 1
    contains "$1: names the problem" "$OUT" "$3"
}
run "$B" check-report.sh --report "$GOOD"
expect "a valid report: exit 0" "$RC" 0
report_case "an unknown verdict" 's/ENTRY | untrusted/ENTRY | maybe/' "is not a verdict"
report_case "no reason on a no-action-warranted entry" 's/ | cold file: untouched for two years//' "malformed ENTRY"
report_case "a fix with no outcome line" '/^FIXED/d' "is a fix with no FIXED or UNPROVEN line"
report_case "FIXED on a test-first entry" 's/^FIXED | src\/covered.rs:10 covered_fn/FIXED | src\/uncovered.rs:20 bare_fn/' "only a fix is edited"
report_case "FIXED without a re-measurement" 's/remeasured: resolved 6 -> gone/remeasured: unchanged 6 -> 6/' "malformed FIXED"
report_case "FIXED with failing tests" 's/0 failed (of 12)/1 failed (of 12)/' "not a pass"
report_case "FIXED over an empty test run" 's/12 passed, 0 failed (of 12)/0 passed, 0 failed (of 0)/' "not a pass"
report_case "FIXED with no test run" 's/ | tests: cargo test | 12 passed, 0 failed (of 12)//' "malformed FIXED"
report_case "an aggregate-signal claim" 's/^## .*//;$a\
Overall the signal went up by 12 points.' "aggregate-signal claim"
report_case "a score claim" '$a\
Quality score: 91.' "aggregate-signal claim"
report_case "the number 10000" '$a\
Now 9980 of 10000.' "aggregate-signal claim"
printf 'ENTRY | fix | src/covered.rs:10 covered_fn | trusted, covered\nUNPROVEN | src/covered.rs:10 covered_fn | still worsened\n' >"$WORK/r3.md"
run "$B" check-report.sh --report "$WORK/r3.md"
expect "an UNPROVEN fix is a valid outcome (exit 0)" "$RC" 0
printf 'ENTRY | fix | src/covered.rs:10 covered_fn | trusted\nFIXED | src/covered.rs:10 covered_fn | remeasured: cleared 6 -> gone | tests: cargo test | 3 passed, 0 failed (of 3)\nUNPROVEN | src/covered.rs:10 covered_fn | and again\n' >"$WORK/r4.md"
run "$B" check-report.sh --report "$WORK/r4.md"
expect "a fix with two outcome lines: exit 1" "$RC" 1
printf 'a report with no verdicts at all\n' >"$WORK/r5.md"
run "$B" check-report.sh --report "$WORK/r5.md"
expect "a report with neither ENTRY nor NO ENTRIES: exit 1" "$RC" 1
printf 'NO ENTRIES\nENTRY | fix | src/a.rs:1 a | trusted\n' >"$WORK/r6.md"
run "$B" check-report.sh --report "$WORK/r6.md"
expect "NO ENTRIES beside an ENTRY: exit 1" "$RC" 1
run "$B" check-report.sh --report "$WORK/missing.md"
expect "an unreadable report: exit 3" "$RC" 3
run "$B" check-report.sh
expect "no --report: exit 2" "$RC" 2

# --- 7. a clean branch ------------------------------------------------------------------------

echo "== a clean branch: no entries, no edit"
for B in "${BASHES[@]}"; do
    T="[$B]"
    git_ checkout -q main 2>/dev/null
    git_ checkout -q -- . 2>/dev/null
    STATE="$WORK/clean.state"
    run "$B" edit-scope.sh begin --state "$STATE"
    STUB_REVIEW=review-clean run "$B" review-delta.sh --out "$WORK/clean.json"
    expect "$T a clean review: exit 0" "$RC" 0
    contains "$T ... 0 entries to judge" "$OUT" "0 entries to judge"
    contains "$T ... the denominator shows the 10 counted entries, none listed" "$ERR" "0 listed of 10 offender and violation entries counted (10 unchanged"
    run "$B" coverage-lookup.sh --delta "$WORK/clean.json" --out "$WORK/clean.cov.json"
    expect "$T coverage over no entries: exit 0" "$RC" 0
    contains "$T ... says 0 entries" "$(last_line "$ERR")" "denominator: 0 entries"
    run "$B" trust-gate.sh --delta "$WORK/clean.json" --coverage "$WORK/clean.cov.json" --out "$WORK/clean.triage.json"
    expect "$T the gate over no entries: no rows" "$OUT" ""
    contains "$T ... denominator" "$(last_line "$ERR")" "0 entries gated: none"
    printf '## logos-review\n\nNO ENTRIES\n' >"$WORK/clean.md"
    run "$B" check-report.sh --report "$WORK/clean.md"
    expect "$T the report 'NO ENTRIES' is valid" "$RC" 0
    run "$B" edit-scope.sh check --state "$STATE"
    expect "$T no edit: the scope holds with nothing allowed" "$RC" 0
    contains "$T ... denominator: nothing changed" "$(last_line "$ERR")" "0 changed since begin, 0 allowed; 0 outside"
    run "$B" merge-base.sh
    contains "$T the branch is identical to its merge-base, so the review need not run" "$OUT" "identical=yes"
done

# --- 8. debt mode and the skill's files ----------------------------------------------------------

echo "== debt-list.sh"
B="${BASHES[0]}"
printf '{"recorded":false,"basis":"","units":[],"outliers":[]}\n' >"$FX/improvements.json"
run "$B" debt-list.sh --out "$WORK/debt.json"
expect "an unrecorded list: exit 1" "$RC" 1
contains "... says 'not recorded', and that it is not clean" "$OUT" "not recorded"
contains "... denominator: 0 units" "$ERR" "0 units"
cat >"$FX/improvements.json" <<'EOF'
{"recorded":true,"basis":"estimate","outliers":[],"units":[
 {"kind":"function","node_id":"src/`covered.rs`/covered_fn().","name":"covered_fn","file":"src/covered.rs","line":10,"dimensions":["nesting"],"points":159,"detail":"nesting depth 6","outliers":["nesting"]},
 {"kind":"function","node_id":"src/`uncovered.rs`/bare_fn().","name":"bare_fn","file":"src/uncovered.rs","line":20,"dimensions":["nesting"],"points":40,"detail":"nesting depth 5","outliers":[]},
 {"kind":"function","node_id":"src/`thin.py`/thin_fn().","name":"thin_fn","file":"src/thin.py","line":5,"dimensions":["nesting"],"points":20,"detail":"nesting depth 5","outliers":[]},
 {"kind":"function","node_id":"src/`dirty.rs`/dirty_fn().","name":"dirty_fn","file":"src/dirty.rs","line":2,"dimensions":["conciseness"],"points":10,"detail":"complexity 12","outliers":[]}]}
EOF
printf '{"languages":[{"name":"rust","extensions":["rs"]},{"name":"python","extensions":["py","pyi"]}]}\n' >"$FX/languages.json"
printf '{"resolution_by_language":[{"language":"rust","calls":{"cross_file_absence":null}},{"language":"python","calls":{"cross_file_absence":{"cause":"same-file-only"}}}]}\n' >"$FX/status.json"
printf 'fn dirty_fn() {}\n' >"$REPO/src/dirty.rs"
run "$B" debt-list.sh --out "$WORK/debt.json"
expect "a recorded list: exit 0" "$RC" 0
contains "... summary" "$OUT" "debt list: 4 unit(s)"
contains "... denominator" "$ERR" "4 unit(s) of the recorded where-to-start list (at most 10); 2 untrusted"
expect "trust: a Python unit carries the language's cause, a dirty file reads stale, the rest resolved" \
    "$(jget "$WORK/debt.json" '",".join(e["trust"] for e in d["entries"])')" "resolved,resolved,same-file-only,stale"
# the ids resolve through the impact fixtures keyed by the id (the stub's answer; the
# 1.20.5 binary resolves only the bare name, which the coverage layer above covers).
run "$B" coverage-lookup.sh --delta "$WORK/debt.json" --out "$WORK/debt.cov.json"
run "$B" trust-gate.sh --delta "$WORK/debt.json" --coverage "$WORK/debt.cov.json" --out "$WORK/debt.triage.json"
expect "debt mode: the same gate -> fix, test-first, untrusted, untrusted" \
    "$(jget "$WORK/debt.triage.json" '",".join(e["proposed"] for e in d["entries"])')" "fix,test-first,untrusted,untrusted"
# debt re-measure: the unit is matched to the offender with its node_id and dimension.
STUB_REVIEW=review-resolved run "$B" remeasure.sh --triage "$WORK/debt.triage.json" --out "$WORK/debt.after.json" --entry e1
expect "debt mode: a fix is proven by the delta's resolved entry (exit 0)" "$RC" 0
contains "... cites it" "$OUT" "proven: e1 covered_fn"
STUB_REVIEW=review-gone run "$B" remeasure.sh --triage "$WORK/debt.triage.json" --out "$WORK/debt.after2.json" --entry e1
expect "debt mode: an offender that is simply not listed is not proven (exit 1)" "$RC" 1
rm -f "$REPO/src/dirty.rs"

# --- 8. the review round: branches and repros the first suite did not reach ---------------

echo "== the delta's other entry kinds, and the gate's fallbacks"
B="${BASHES[0]}"
cat >"$FX/review-rich.json" <<'EOF'
{"base":"main","base_origin":"default-branch","head":"h","merge_base":"m","measured":true,"not_measured_because":null,"before_state":"shadow-index",
 "offenders":[
  {"dimension":"acyclicity","status":"new","node_id":"a/`x.rs`/f().","name":"f","file":"a/x.rs","line":2,"measure":"cycle_size","before":null,"after":4,"threshold":null,"detail":"4 symbols across 2 directories: a, b","trust":"resolved"},
  {"dimension":"depth","status":"new","node_id":null,"name":"chain","file":"","line":null,"measure":"chain_length","before":null,"after":7,"threshold":6,"detail":"chain","trust":"resolved"},
  {"dimension":"nesting","status":"new","node_id":"src/`covered.rs`/covered_fn().","name":"covered_fn","file":"src/covered.rs","line":10,"measure":"nesting_depth","before":null,"after":6,"threshold":4,"detail":"d","trust":"resolved"},
  {"dimension":"nesting","status":"new","node_id":"src/`uncovered.rs`/bare_fn().","name":"bare_fn","file":"src/uncovered.rs","line":20,"measure":"nesting_depth","before":null,"after":6,"threshold":4,"detail":"d"},
  {"dimension":"nesting","status":"new","node_id":"src/`garbage.rs`/g().","name":"g","file":"src/garbage.rs","line":1,"measure":"nesting_depth","before":null,"after":6,"threshold":4,"detail":"d","trust":"resolved"},
  {"dimension":"nesting","status":"new","node_id":"x/`n.rs`/h().","name":"h","file":"","line":null,"measure":"nesting_depth","before":null,"after":6,"threshold":4,"detail":"d","trust":"resolved"}],
 "offender_counts":{"new":6,"worsened":0,"improved":0,"resolved":0,"unchanged":0},
 "violations":[
  {"rule":"max_cc","rule_type":"constraint","status":"new","subject":"src/`covered.rs`/covered_fn().","file":"src/covered.rs","before":null,"after":12,"threshold":10,"message":"cc 12","trust":"resolved"},
  {"rule":"no_god_files","rule_type":"constraint","status":"new","subject":"src/covered.rs","file":"src/covered.rs","before":null,"after":null,"threshold":null,"message":"god file","trust":"resolved"}],
 "violation_counts":{"new":2,"worsened":0,"improved":0,"resolved":0,"unchanged":0},
 "introduced_clone_groups":[{"size":3,"members":[
  {"node_id":"src/`covered.rs`/dup_a().","name":"dup_a","file":"src/covered.rs","line":3,"newly_duplicated":true,"trust":"resolved"},
  {"node_id":"src/`covered.rs`/dup_b().","name":"dup_b","file":"src/covered.rs","line":6,"newly_duplicated":false,"trust":"resolved"}]}],
 "stale_files":[],"sections":{},"notes":[]}
EOF
printf 'not json at all\n' >"$FX/affected.src_garbage_rs.json"
: >"$CALLS"
STUB_REVIEW=review-rich run "$B" review-delta.sh --out "$WORK/rich.json"
contains "a delta with violations and a clone group: 9 entries (6 offenders, 2 violations, 1 clone member)" "$OUT" "9 entries to judge (offenders 6, violations 2, clone members 1)"
contains "... the clone member already duplicated before the branch is counted, not listed" "$ERR" "1 clone-group member(s) already cloned before the branch"
run "$B" coverage-lookup.sh --delta "$WORK/rich.json" --out "$WORK/rich.cov.json" --depth 3
expect "coverage over the rich delta: exit 0 (a non-JSON affected answer does not crash it)" "$RC" 0
idcov() { python3 -I - "$1" <<'PYEOF'
import json, sys
print(",".join(c["id"] + ":" + c["coverage"] for c in json.load(open(sys.argv[1]))["coverage"]))
PYEOF
}
expect "coverage per entry: structural, no-symbol, covered, uncovered, unknown (non-JSON), unknown (no file), covered (violation symbol), no-symbol (file rule), unknown (unresolved clone member)" \
    "$(idcov "$WORK/rich.cov.json")" "e1:no-symbol,e2:no-symbol,e3:covered,e4:uncovered,e5:unknown,e6:unknown,e7:covered,e8:no-symbol,e9:unknown"
contains "... --depth is forwarded to impact" "$(grep '^impact' "$CALLS" | head -1)" "--depth 3"
expect "... one affected call per file, not per entry (covered.rs serves 2 entries)" "$(grep -c '^affected' "$CALLS")" 3
# 3 affected + 3 impact by id + 1 impact by bare name (dup_a's id resolves to nothing).
contains "... the denominator counts the calls" "$(last_line "$ERR")" "7 logos call(s)"
run "$B" trust-gate.sh --delta "$WORK/rich.json" --coverage "$WORK/rich.cov.json" --out "$WORK/rich.tri.json"
idprop() { python3 -I - "$1" <<'PYEOF'
import json, sys
print(",".join(e["id"] + ":" + e["proposed"] for e in json.load(open(sys.argv[1]))["entries"]))
PYEOF
}
expect "GATE over every kind: a cycle and a chain have no symbol; a missing trust key is untrusted; a violation with a symbol is a fix; a file rule has none; an unresolved clone member is test-first" \
    "$(idprop "$WORK/rich.tri.json")" "e1:no-action-warranted,e2:no-action-warranted,e3:fix,e4:untrusted,e5:test-first,e6:test-first,e7:fix,e8:no-action-warranted,e9:test-first"
expect "... the acyclicity entry reached no-action because it names no symbol, not because of its status" \
    "$(jget "$WORK/rich.tri.json" 'd["entries"][0]["symbol"]')" None
expect "... a clone member's key names its symbol" "$(jget "$WORK/rich.tri.json" 'd["entries"][8]["key"]')" 'clone|uniqueness||src/`covered.rs`/dup_a().'
expect "... a violation's key carries its rule" "$(jget "$WORK/rich.tri.json" 'd["entries"][6]["key"].split("|")[:2]')" "['violation', 'max_cc']"
contains "... the table row has all nine columns, in order" "$(printf '%s\n' "$OUT" | sed -n 3p)" "$(printf 'e3\tfix?\tnew\tnesting\tcovered_fn\tsrc/covered.rs:10\tcovered\tresolved\ttrusted and covered by tests; confirm it is worth the change')"
contains "... a missing trust key reads 'missing' in the reason" "$OUT" "trust label 'missing'"

# a coverage file that lacks an entry, or was made for another delta
printf '{"coverage":[]}\n' >"$WORK/nocov.json"
run "$B" trust-gate.sh --delta "$WORK/delta.json" --coverage "$WORK/nocov.json" --out "$WORK/nocov.tri.json"
expect "a row with no coverage answer reads unknown, so a trusted entry is test-first, never fix" "$(verdicts "$WORK/nocov.tri.json")" "covered_fn:test-first,bare_fn:test-first,thin_fn:untrusted"
run "$B" trust-gate.sh --delta "$WORK/rich.json" --coverage "$WORK/cov.json" --out "$WORK/stalecov.tri.json"
expect "a coverage file computed for another delta does not land on these entries (keys differ): no fix" \
    "$(jget "$WORK/stalecov.tri.json" 'sum(1 for e in d["entries"] if e["proposed"] == "fix")')" 0
printf '[]\n' >"$WORK/list.json"
run "$B" trust-gate.sh --delta "$WORK/list.json" --coverage "$WORK/cov.json" --out "$WORK/x.json"
expect "a delta that is not a JSON object: exit 3, not a traceback" "$RC" 3
contains "... and the message says what is wrong with it" "$ERR" "is not a JSON object"
run "$B" trust-gate.sh --delta "$WORK/missing.json" --coverage "$WORK/cov.json" --out "$WORK/x.json"
expect "a delta that cannot be read: exit 3" "$RC" 3
run "$B" trust-gate.sh --delta "$WORK/delta.json" --coverage "$WORK/cov.json" --out "$WORK/no-such-dir/x.json"
expect "an --out in a missing directory: exit 3, not the 'other answer'" "$RC" 3

echo "== a summary with every count"
cat >"$FX/review-counts.json" <<'EOF'
{"base":"main","merge_base":"m","measured":true,"before_state":"recorded-snapshot",
 "offenders":[{"dimension":"nesting","status":"new","node_id":"a","name":"a","file":"a.rs","line":1,"trust":"resolved"}],
 "offender_counts":{"new":1,"worsened":0,"improved":2,"resolved":3,"unchanged":4},
 "violations":[],"violation_counts":{"new":1,"worsened":0,"improved":0,"resolved":0,"unchanged":5},
 "introduced_clone_groups":[],"stale_files":["a.rs"]}
EOF
STUB_REVIEW=review-counts run "$B" review-delta.sh --out "$WORK/counts.json"
contains "one entry reads in the singular, with the stale count" "$OUT" "1 entry to judge (offenders 1, violations 0, clone members 0); stale files 1"
contains "... the denominator counts improved, resolved and the violations' own counts (16, of which 9 unchanged)" "$ERR" "1 listed of 16 offender and violation entries counted (9 unchanged"

echo "== check-report.sh: the rest of its rules"
TRI="$WORK/triage.json"
cat >"$WORK/t.md" <<'EOF'
ENTRY | fix | src/covered.rs:10 covered_fn | trusted, covered
ENTRY | test-first | src/uncovered.rs:20 bare_fn | no test reach
ENTRY | untrusted | src/thin.py:5 thin_fn | trust label 'same-file-only'
FIXED | src/covered.rs:10 covered_fn | remeasured: improved 6 -> 4 | tests: cargo test | 12 passed, 0 failed (of 13)
EOF
run "$B" check-report.sh --report "$WORK/t.md" --triage "$TRI"
expect "a report held to its triage: valid, with 'improved' and skipped tests (12 passed of 13) accepted (exit 0)" "$RC" 0
contains "... the denominator says it was held to the triage" "$(last_line "$ERR")" "held to the triage"
run "$B" check-report.sh --report "$WORK/t.md"
contains "... and says so when it was not" "$(last_line "$ERR")" "not compared with the gate"
tri_case() { # label sed-expression expected-substring
    sed "$2" "$WORK/t.md" >"$WORK/tc.md"
    run "$B" check-report.sh --report "$WORK/tc.md" --triage "$TRI"
    expect "$1: exit 1" "$RC" 1
    contains "$1: names the problem" "$OUT" "$3"
}
tri_case "a test-first row judged fix" 's/ENTRY | test-first | src\/uncovered.rs:20 bare_fn/ENTRY | fix | src\/uncovered.rs:20 bare_fn/' "the gate allows ['test-first']"
tri_case "an untrusted row judged fix" 's/ENTRY | untrusted/ENTRY | fix/' "the gate allows ['untrusted']"
tri_case "a triage row omitted from the report" '/thin_fn | trust/d' "has no ENTRY line"
tri_case "a subject that is not a triage row" 's/src\/thin.py:5 thin_fn/src\/other.py:5 other_fn/' "is not a row of the triage"
sed 's/ENTRY | fix | src\/covered.rs:10 covered_fn | trusted, covered/ENTRY | no-action-warranted | src\/covered.rs:10 covered_fn | cold file/;/^FIXED/d' "$WORK/t.md" >"$WORK/td.md"
run "$B" check-report.sh --report "$WORK/td.md" --triage "$TRI"
expect "a proposed fix downgraded to no-action-warranted is allowed (exit 0)" "$RC" 0

printf 'NO ENTRIES\nENTRY | no-action-warranted | src/a.rs:1 a | cold file\n' >"$WORK/n1.md"
run "$B" check-report.sh --report "$WORK/n1.md"
contains "NO ENTRIES beside an ENTRY alone is refused" "$OUT" "NO ENTRIES and lists an entry"
printf 'NO ENTRIES\nUNPROVEN | src/a.rs:1 a | still worsened\n' >"$WORK/n2.md"
run "$B" check-report.sh --report "$WORK/n2.md"
contains "NO ENTRIES beside an outcome line alone is refused" "$OUT" "claims an edit"
printf 'NO ENTRIESFOO\n' >"$WORK/n3.md"
run "$B" check-report.sh --report "$WORK/n3.md"
expect "near miss: NO ENTRIESFOO is not NO ENTRIES" "$RC" 1
printf 'ENTRY|fix|src/a.rs:1 a|r\n' >"$WORK/n4.md"
run "$B" check-report.sh --report "$WORK/n4.md"
contains "a verdict line missing its spaces is malformed, never silently prose" "$OUT" "malformed ENTRY"
printf 'ENTRY | fix | src/a.rs:1 a | r\nENTRY | untrusted | src/a.rs:1 a | r2\nUNPROVEN | src/a.rs:1 a | x\n' >"$WORK/n5.md"
run "$B" check-report.sh --report "$WORK/n5.md"
contains "two ENTRY lines for one subject are refused" "$OUT" "two ENTRY lines"
pass_case() { # label subject-suffix
    printf 'ENTRY | fix | src/a.rs:1 a | r\nFIXED | src/a.rs:1 a | remeasured: resolved %s | tests: t | %s\n' "$2" "$3" >"$WORK/p.md"
    run "$B" check-report.sh --report "$WORK/p.md"
    expect "$1" "$RC" "$4"
}
pass_case "a FIXED with 5 passed, 0 failed (of 4) is refused (more passed than ran)" "6 -> 4" "5 passed, 0 failed (of 4)" 1
pass_case "a FIXED with 0 passed, 0 failed (of 3) is refused" "6 -> 4" "0 passed, 0 failed (of 3)" 1
pass_case "a FIXED with 3 passed, 0 failed (of 0) is refused" "6 -> 4" "3 passed, 0 failed (of 0)" 1
pass_case "a FIXED whose re-measurement is not 'before -> after' is refused" "zzz" "3 passed, 0 failed (of 3)" 1
pass_case "a FIXED 'before -> after' with no spaces is refused" "9->4" "3 passed, 0 failed (of 3)" 1
pass_case "a well-formed FIXED is valid" "6 -> 4" "3 passed, 0 failed (of 3)" 0
printf 'ENTRY | fix | src/a.rs:1 operator| | r\nFIXED | src/a.rs:1 operator| | remeasured: resolved 6 -> 4 | tests: pytest -k "a|b" | 3 passed, 0 failed (of 3)\n' >"$WORK/p2.md"
run "$B" check-report.sh --report "$WORK/p2.md"
expect "a name with a pipe, and a test command with one, are accepted" "$RC" 0
for sentence in "Aggregate Signal went up." "The signals improved." "Scores rose." "Now 10,000 of 10,000." "The aggregate moved."; do
    printf 'NO ENTRIES\n%s\n' "$sentence" >"$WORK/s.md"
    run "$B" check-report.sh --report "$WORK/s.md"
    expect "a signal claim in prose is refused: $sentence" "$RC" 1
done
printf 'ENTRY | fix | logos-core/src/quality/signal.rs:10 score | a normal reason\nFIXED | logos-core/src/quality/signal.rs:10 score | remeasured: resolved 6 -> 4 | tests: cargo test | 3 passed, 0 failed (of 3)\n' >"$WORK/s2.md"
run "$B" check-report.sh --report "$WORK/s2.md"
expect "a file called signal.rs and a function called score are subjects, not claims (exit 0)" "$RC" 0
printf 'ENTRY | fix | src/a.rs:1 a | removes the signal handler\n' >"$WORK/s3.md"
run "$B" check-report.sh --report "$WORK/s3.md"
expect "the word in a reason is still a claim" "$RC" 1
printf '\xff\xfe\n' >"$WORK/bin.md"
run "$B" check-report.sh --report "$WORK/bin.md"
expect "a report that is not text: exit 3, not the 'other answer'" "$RC" 3

echo "== remeasure.sh: the rest of the ways a fix is or is not proven"
rm2() { STUB_REVIEW="$1" run "$B" remeasure.sh --triage "$WORK/triage.json" --out "$WORK/after-$1.json" "${@:2}"; }
rm2 review-resolved --entry e1 --entry e2
expect "two entries, one proven and one not: exit 1" "$RC" 1
contains "... the proven one is printed" "$OUT" "proven: e1 covered_fn"
contains "... and the one still new" "$OUT" "not proven: e2 bare_fn (src/uncovered.rs:20): still new"
contains "... both files were synced, in one call" "$(grep '^sync' "$CALLS" | tail -1)" "sync -- src/covered.rs src/uncovered.rs"
rm2 review-resolved --entry e1 --entry e1
contains "the same entry named twice syncs its file once" "$(grep '^sync' "$CALLS" | tail -1)" "sync -- src/covered.rs"
lacks "... and not twice" "$(grep '^sync' "$CALLS" | tail -1)" "covered.rs src/covered.rs"
: >"$CALLS"
rm2 review-resolved --entry e9
expect "an unknown entry stops before any sync or run (exit 2)" "$RC" 2
expect "... and no logos call was made" "$(grep -c '^sync\|^review' "$CALLS")" 0
STUB_REVIEW_RC=7 rm2 review-resolved --entry e1
expect "a review that fails during a re-run is an environment error (exit 3)" "$RC" 3
rm2 review-resolved --entry e1 --base release/2
contains "--base reaches the re-run" "$(grep '^review' "$CALLS" | tail -1)" "--base release/2"

python3 -I - "$FX" <<'PYEOF'
import json, sys
fx = sys.argv[1]
base = json.load(open(fx + "/review.json"))
def variant(name, mutate):
    d = json.loads(json.dumps(base))
    mutate(d)
    json.dump(d, open(f"{fx}/{name}.json", "w"))
# covered_fn gone, a new helper offender in its place (the edit moved the offence)
def moved(d):
    helper = dict(d["offenders"][1], name="helper", node_id="src/`covered.rs`/helper().", file="src/covered.rs", line=12)
    d["offenders"] = [helper, d["offenders"][2]]
variant("review-moved", moved)
# bare_fn now crosses on its span: one entry gone, another new
def flipped(d):
    d["offenders"][1]["measure"] = "span"
variant("review-flipped", flipped)
# covered_fn resolved, but its language now reads same-file-only
def thin(d):
    d["offenders"][0].update(status="resolved", after=None, trust="same-file-only")
variant("review-thin-after", thin)
def still_new(d):
    d["offenders"][0].update(status="new")
variant("review-new-after", still_new)
def unchanged(d):
    d["offenders"][0].update(status="unchanged")
variant("review-unchanged-after", unchanged)
PYEOF
rm2 review-moved --entry e2
expect "a new entry that vanished while a new helper offender appeared: not cleared (exit 1)" "$RC" 1
contains "... the cleared line is there" "$OUT" "cleared: e2 bare_fn"
contains "... and so is what the edit added" "$OUT" "the re-run lists an offender the triage did not have: src/covered.rs:12 helper"
contains "... the denominator counts it" "$(last_line "$ERR")" "1 unexplained"
rm2 review-flipped --entry e2
expect "a container that now crosses on another measure is not cleared (exit 1)" "$RC" 1
contains "... says it is still listed under another measure" "$OUT" "still listed under another measure (span)"
rm2 review-thin-after --entry e1
expect "a re-run label of same-file-only is not proven (exit 1)" "$RC" 1
contains "... says so" "$OUT" "labels it 'same-file-only'"
rm2 review-new-after --entry e1
expect "an entry now 'new' is not proven (exit 1)" "$RC" 1
contains "... still new" "$OUT" "still new"
rm2 review-unchanged-after --entry e1
expect "an entry still 'unchanged' is not proven (exit 1)" "$RC" 1
contains "... still unchanged" "$OUT" "still unchanged"

echo "== remeasure.sh in debt mode: the match is by node_id and dimension; a name-only unit by name, file and dimension"
debt_case() { # label dimension file expected-rc identity symbol
    printf '{"mode":"debt","entries":[{"kind":"debt","identity":"%s","dimension":"%s","status":"debt","symbol":"%s","name":"covered_fn","file":"%s","line":10,"measure":"function","detail":"d","trust":"resolved"}]}\n' "${5:-name}" "$2" "${6:-covered_fn}" "$3" >"$WORK/dd.json"
    bash "$SCR/trust-gate.sh" --delta "$WORK/dd.json" --coverage "$WORK/nocov.json" --out "$WORK/dd.tri.json" >/dev/null 2>&1
    STUB_REVIEW=review-resolved run "$B" remeasure.sh --triage "$WORK/dd.tri.json" --out "$WORK/dd.after.json" --entry e1
    expect "$1" "$RC" "$4"
}
debt_case "a name-only unit in the offender's own dimension and file is proven" nesting src/covered.rs 0
debt_case "a name-only unit of another dimension is not the offender (near miss)" conciseness src/covered.rs 1
debt_case "a name-only unit of the same name in another file is not the offender (near miss)" nesting src/other.rs 1
debt_case "a unit with the offender's node_id and dimension is proven" nesting src/covered.rs 0 node_id 'src/`covered.rs`/covered_fn().'
debt_case "a unit with the offender's node_id in another dimension is not the offender (near miss)" conciseness src/covered.rs 1 node_id 'src/`covered.rs`/covered_fn().'
debt_case "a unit of the same name whose node_id is another file's is not the offender (near miss)" nesting src/other.rs 1 node_id 'src/`other.rs`/covered_fn().'

echo "== debt mode names a unit by its node_id (string-id fixtures; groups; the integer/null fallback)"
printf '{"languages":[{"name":"rust","extensions":["rs"]},{"name":"python","extensions":["py","pyi"]}]}\n' >"$FX/languages.json"
printf '{"resolution_by_language":[{"language":"rust","calls":{"cross_file_absence":null}},{"language":"python","calls":{"cross_file_absence":{"cause":"same-file-only"}}}]}\n' >"$FX/status.json"
NEAR_ID='"src/`covered.rs`/Other#covered_fn()."'
# The delta after a fix attempt, one fixture per way the same name can appear beside the unit.
review_doc "[$(offender resolved "$NEAR_ID" covered_fn src/covered.rs 50 5 null resolved)]" >"$FX/review-nearmiss.json"
review_doc "[$(offender resolved "$NEAR_ID" covered_fn src/covered.rs 50 5 null resolved),$(offender worsened "$COVERED_ID" covered_fn src/covered.rs 10 5 6 resolved)]" >"$FX/review-sibling-first-resolved.json"
review_doc "[$(offender worsened "$NEAR_ID" covered_fn src/covered.rs 50 5 6 resolved),$(offender resolved "$COVERED_ID" covered_fn src/covered.rs 10 5 null resolved)]" >"$FX/review-sibling-first-worse.json"
for B in "${BASHES[@]}"; do
    T="[$B]"
    cat >"$FX/improvements.json" <<'EOF'
{"recorded":true,"basis":"estimate","outliers":[],"units":[
 {"kind":"function","node_id":"src/`covered.rs`/covered_fn().","name":"covered_fn","file":"src/covered.rs","line":10,"dimensions":["nesting"],"points":159,"detail":"nesting depth 6","outliers":[]},
 {"kind":"function","node_id":"src/`uncovered.rs`/bare_fn().","name":"bare_fn","file":"src/uncovered.rs","line":20,"dimensions":["nesting"],"points":40,"detail":"nesting depth 5","outliers":[]},
 {"kind":"function","node_id":"src/`thin.py`/thin_fn().","name":"thin_fn","file":"src/thin.py","line":5,"dimensions":["nesting"],"points":20,"detail":"nesting depth 5","outliers":[]},
 {"kind":"clone_group","node_id":"clone_group:0123456789abcdef","name":"covered_fn","file":"src/covered.rs","line":10,"dimensions":["uniqueness"],"points":30,"detail":"clone group #1 · 3 members × 7 lines","outliers":[]},
 {"kind":"duplicate_group","node_id":"duplicate_group:fedcba9876543210","name":"bare_fn","file":"src/uncovered.rs","line":20,"dimensions":["redundancy"],"points":10,"detail":"duplicate group #2 · 2 members × 9 lines","outliers":[]}]}
EOF
    run "$B" debt-list.sh --out "$WORK/debt-id.json"
    expect "$T string ids: exit 0" "$RC" 0
    contains "$T ... summary names no fallback" "$OUT" "debt list: 5 unit(s)"
    lacks "$T ... summary names no fallback" "$OUT" "known by name only"
    contains "$T ... denominator: 3 by node_id, 2 groups, 0 fell back" "$ERR" "by node_id 3, groups naming no single symbol 2, fell back to name, file and dimension 0"
    expect "$T a unit's symbol is its node_id; a group names no symbol" \
        "$(jget "$WORK/debt-id.json" '",".join(str(e["symbol"]) for e in d["entries"])')" \
        'src/`covered.rs`/covered_fn().,src/`uncovered.rs`/bare_fn().,src/`thin.py`/thin_fn().,None,None'
    expect "$T the bare name is kept for reading" \
        "$(jget "$WORK/debt-id.json" '",".join(e["name"] for e in d["entries"])')" "covered_fn,bare_fn,thin_fn,covered_fn,bare_fn"
    expect "$T identities: node_id, node_id, node_id, group, group" \
        "$(jget "$WORK/debt-id.json" '",".join(e["identity"] for e in d["entries"])')" "node_id,node_id,node_id,group,group"
    contains "$T a clone group carries its own reason" "$(jget "$WORK/debt-id.json" 'd["entries"][3]["group_reason"]')" "clone group names no single symbol"
    contains "$T a duplicate group carries its own reason" "$(jget "$WORK/debt-id.json" 'd["entries"][4]["group_reason"]')" "duplicate group names no single symbol"
    expect "$T a group keeps its own id, apart from any symbol" \
        "$(jget "$WORK/debt-id.json" 'd["entries"][3]["group"]')" "clone_group:0123456789abcdef"
    : >"$CALLS"
    run "$B" coverage-lookup.sh --delta "$WORK/debt-id.json" --out "$WORK/debt-id.cov.json"
    expect "$T coverage by id: covered, uncovered, covered, then the two groups name no symbol" \
        "$(jget "$WORK/debt-id.cov.json" '",".join(c["coverage"] for c in d["coverage"])')" "covered,uncovered,covered,no-symbol,no-symbol"
    lacks "$T ... the id resolved, so no bare-name call was made" "$(cat "$CALLS")" "impact covered_fn --depth"
    contains "$T ... a clone group's note is its own reason" "$(jget "$WORK/debt-id.cov.json" 'd["coverage"][3]["note"]')" "clone group names no single symbol"
    contains "$T ... a duplicate group's note is its own reason" "$(jget "$WORK/debt-id.cov.json" 'd["coverage"][4]["note"]')" "duplicate group names no single symbol"
    # A unit known by id is never made ambiguous by a same-named node of its file: the id tells them apart.
    impact_as 'src/`covered.rs`/covered_fn().' 'src/`covered.rs`/covered_fn().' src/covered.rs tests/covered_test.rs \
        '[{"symbol":"logos . repo . src/`covered.rs`/Other#covered_fn().","name":"covered_fn","kind":"function","file":"src/covered.rs","line":50}]' \
        >"$FX/impact.$(k 'src/`covered.rs`/covered_fn().').json"
    run "$B" coverage-lookup.sh --delta "$WORK/debt-id.json" --out "$WORK/debt-id.cov-alt.json"
    expect "$T a string-id unit with a same-file same-name alternative is still covered by its own id" \
        "$(jget "$WORK/debt-id.cov-alt.json" 'd["coverage"][0]["coverage"]')" covered
    impact 'src/`covered.rs`/covered_fn().' src/covered.rs tests/covered_test.rs >"$FX/impact.$(k 'src/`covered.rs`/covered_fn().').json"
    contains "$T ... denominator states the identities" "$ERR" "by node_id 3, groups naming no single symbol 2"
    run "$B" trust-gate.sh --delta "$WORK/debt-id.json" --coverage "$WORK/debt-id.cov.json" --out "$WORK/debt-id.triage.json"
    expect "$T the gate: fix, test-first, untrusted, then the groups no-action-warranted" \
        "$(jget "$WORK/debt-id.triage.json" '",".join(e["proposed"] for e in d["entries"])')" "fix,test-first,untrusted,no-action-warranted,no-action-warranted"
    contains "$T ... a group row's reason is its own" "$(jget "$WORK/debt-id.triage.json" 'd["entries"][3]["reason"]')" "clone group names no single symbol"
    contains "$T ... denominator states the identities" "$ERR" "by node_id 3, groups naming no single symbol 2"

    STUB_REVIEW=review-resolved run "$B" remeasure.sh --triage "$WORK/debt-id.triage.json" --out "$WORK/debt-id.after.json" --entry e1
    expect "$T re-measure by id: the unit's own resolved entry proves it (exit 0)" "$RC" 0
    contains "$T ... cites it" "$OUT" "proven: e1 covered_fn"
    lacks "$T ... an id match carries no name-match suffix" "$OUT" "matched by name"
    contains "$T ... denominator: one unit known by id" "$ERR" "by node_id 1, groups naming no single symbol 0, fell back to name, file and dimension 0"
    STUB_REVIEW=review-nearmiss run "$B" remeasure.sh --triage "$WORK/debt-id.triage.json" --out "$WORK/debt-id.after2.json" --entry e1
    expect "$T near miss: a same-named symbol of another id, in the same file and dimension, is not the unit (exit 1)" "$RC" 1
    contains "$T ... the unit is not proven" "$OUT" "not proven: e1 covered_fn"
    STUB_REVIEW=review-sibling-first-resolved run "$B" remeasure.sh --triage "$WORK/debt-id.triage.json" --out "$WORK/debt-id.after3.json" --entry e1
    expect "$T two same-named symbols in one file: the first, resolved, is not the unit; the unit is still worsened (exit 1)" "$RC" 1
    contains "$T ... read from the unit's own entry" "$OUT" "still worsened"
    STUB_REVIEW=review-sibling-first-worse run "$B" remeasure.sh --triage "$WORK/debt-id.triage.json" --out "$WORK/debt-id.after4.json" --entry e1
    expect "$T two same-named symbols in one file: the unit's own entry is the second, and resolved (exit 0)" "$RC" 0
    STUB_REVIEW=review-resolved run "$B" remeasure.sh --triage "$WORK/debt-id.triage.json" --out "$WORK/debt-id.after5.json" --entry e4
    expect "$T a group is never re-measured against an offender (exit 1)" "$RC" 1
    contains "$T ... with its own reason" "$OUT" "clone group names no single symbol"
    contains "$T ... and the group's detail" "$OUT" "3 members"
    contains "$T ... denominator: one group asked" "$ERR" "by node_id 0, groups naming no single symbol 1"
    # A resolved offender of the group's own name, file and dimension is still not the group.
    UNIQ="$(offender resolved "$COVERED_ID" covered_fn src/covered.rs 10 5 null resolved | sed 's/"dimension":"nesting"/"dimension":"uniqueness"/')"
    review_doc "[$UNIQ]" >"$FX/review-uniq.json"
    contains "$T ... (the fixture offender is in the group's dimension)" "$(cat "$FX/review-uniq.json")" '"dimension":"uniqueness"'
    STUB_REVIEW=review-uniq run "$B" remeasure.sh --triage "$WORK/debt-id.triage.json" --out "$WORK/debt-id.after6.json" --entry e4
    expect "$T near miss: an offender of the group's name, file and dimension is not the group (exit 1)" "$RC" 1
    contains "$T ... still reported with the group's reason" "$OUT" "clone group names no single symbol"
    # A group in a thin-language file is untrusted first; its own reason is kept apart from the gate's.
    cat >"$FX/improvements.json" <<'EOF'
{"recorded":true,"basis":"estimate","outliers":[],"units":[
 {"kind":"clone_group","node_id":"clone_group:0123456789abcdef","name":"thin_fn","file":"src/thin.py","line":5,"dimensions":["uniqueness"],"points":30,"detail":"clone group #1 · 3 members × 7 lines","outliers":[]}]}
EOF
    run "$B" debt-list.sh --out "$WORK/debt-ug.json"
    run "$B" coverage-lookup.sh --delta "$WORK/debt-ug.json" --out "$WORK/debt-ug.cov.json"
    run "$B" trust-gate.sh --delta "$WORK/debt-ug.json" --coverage "$WORK/debt-ug.cov.json" --out "$WORK/debt-ug.triage.json"
    expect "$T an untrusted group: the gate reads untrusted first" "$(jget "$WORK/debt-ug.triage.json" 'd["entries"][0]["proposed"]')" untrusted
    contains "$T ... the group's own reason survives the triage" "$(jget "$WORK/debt-ug.triage.json" 'd["entries"][0]["group_reason"]')" "clone group names no single symbol"
    STUB_REVIEW=review-resolved run "$B" remeasure.sh --triage "$WORK/debt-ug.triage.json" --out "$WORK/debt-ug.after.json" --entry e1
    contains "$T ... re-measurement states the group's reason" "$OUT" "clone group names no single symbol"
    lacks "$T ... never the trust label spliced into it" "$OUT" "reported, never edited"
    SAME="$(python3 -I -B -c '
import sys
sys.path.insert(0, sys.argv[1])
from delta import make_entry, same_unit
off = make_entry("offender", dimension="uniqueness", symbol="s/`a.rs`/f().", name="f", file="a.rs", status="new")
grp = make_entry("debt", dimension="uniqueness", symbol=None, name="f", file="a.rs", identity="group")
byid = make_entry("debt", dimension="uniqueness", symbol="s/`a.rs`/f().", name="f", file="a.rs", identity="node_id")
other = make_entry("debt", dimension="uniqueness", symbol="s/`a.rs`/Other#f().", name="f", file="a.rs", identity="node_id")
prefixed = make_entry(**__import__("delta").debt_entry({"kind": "function", "node_id": "clone_group:abc", "name": "f", "file": "a.rs", "dimensions": ["uniqueness"]}, set(), {}, {}))
print(same_unit(grp, off), same_unit(off, grp), same_unit(byid, off), same_unit(other, off), same_unit(off, other), prefixed["identity"], prefixed["symbol"])' "$SCR/lib")"
    expect "$T same_unit: a group is no offender, in either order; an id match is; a same-named other id is not" \
        "$SAME" "False False True False False group None"
done

echo "== debt mode against an integer or null id (Logos 1.20.0-1.20.2): the fallback is visible"
B="${BASHES[0]}"
cat >"$FX/improvements.json" <<'EOF'
{"recorded":true,"basis":"estimate","outliers":[],"units":[
 {"kind":"function","node_id":10,"name":"covered_fn","file":"src/covered.rs","line":10,"dimensions":["nesting"],"points":159,"detail":"nesting depth 6","outliers":[]},
 {"kind":"function","node_id":null,"name":"bare_fn","file":"src/uncovered.rs","line":20,"dimensions":["nesting"],"points":40,"detail":"nesting depth 5","outliers":[]},
 {"kind":"function","node_id":"src/`thin.py`/thin_fn().","name":"thin_fn","file":"src/thin.py","line":5,"dimensions":["nesting"],"points":20,"detail":"nesting depth 5","outliers":[]},
 {"kind":"clone_group","node_id":7,"name":"covered_fn","file":"src/covered.rs","line":10,"dimensions":["uniqueness"],"points":30,"detail":"clone group #1 · 3 members × 7 lines","outliers":[]}]}
EOF
run "$B" debt-list.sh --out "$WORK/debt-fb.json"
expect "an integer or null id still lists (exit 0)" "$RC" 0
contains "... the summary counts the fallback" "$OUT" "2 known by name only (no string id)"
contains "... the denominator counts it: 1 by id, 1 group, 2 fell back" "$ERR" "by node_id 1, groups naming no single symbol 1, fell back to name, file and dimension 2"
expect "identities: name, name, node_id, group (an integer group id is still a group)" \
    "$(jget "$WORK/debt-fb.json" '",".join(e["identity"] for e in d["entries"])')" "name,name,node_id,group"
expect "a fallback unit's symbol is its bare name" "$(jget "$WORK/debt-fb.json" 'd["entries"][0]["symbol"]')" covered_fn
impact covered_fn src/covered.rs tests/covered_test.rs >"$FX/impact.covered_fn.json"
run "$B" coverage-lookup.sh --delta "$WORK/debt-fb.json" --out "$WORK/debt-fb.cov.json"
expect "coverage of a fallback unit is by its name: covered" "$(jget "$WORK/debt-fb.cov.json" 'd["coverage"][0]["coverage"]')" covered
contains "... denominator counts the fallback" "$ERR" "fell back to name, file and dimension 2"
# Two same-named nodes in the unit's file and no id to tell them apart: nothing is credited.
impact_as covered_fn "$OWN" src/covered.rs tests/covered_test.rs \
    "[{\"symbol\":\"$SIB\",\"name\":\"covered_fn\",\"kind\":\"function\",\"file\":\"src/covered.rs\",\"line\":50}]" >"$FX/impact.covered_fn.json"
run "$B" coverage-lookup.sh --delta "$WORK/debt-fb.json" --out "$WORK/debt-fb.cov2.json"
expect "near miss: two same-named symbols in the file, no id -> unknown" "$(jget "$WORK/debt-fb.cov2.json" 'd["coverage"][0]["coverage"]')" unknown
contains "... the note says why" "$(jget "$WORK/debt-fb.cov2.json" 'd["coverage"][0]["note"]')" "carries no id"
impact_as covered_fn "$OWN" src/covered.rs tests/covered_test.rs \
    '[{"symbol":"logos . repo . src/`elsewhere.rs`/covered_fn().","name":"covered_fn","kind":"function","file":"src/elsewhere.rs","line":9}]' >"$FX/impact.covered_fn.json"
run "$B" coverage-lookup.sh --delta "$WORK/debt-fb.json" --out "$WORK/debt-fb.cov3.json"
expect "an alternative of that name in another file does not make the fallback unit ambiguous -> covered" \
    "$(jget "$WORK/debt-fb.cov3.json" 'd["coverage"][0]["coverage"]')" covered
impact covered_fn src/covered.rs tests/covered_test.rs >"$FX/impact.covered_fn.json"
run "$B" coverage-lookup.sh --delta "$WORK/debt-fb.json" --out "$WORK/debt-fb.cov.json"
run "$B" trust-gate.sh --delta "$WORK/debt-fb.json" --coverage "$WORK/debt-fb.cov.json" --out "$WORK/debt-fb.triage.json"
contains "the gate's denominator counts the fallback" "$ERR" "fell back to name, file and dimension 2"
STUB_REVIEW=review-resolved run "$B" remeasure.sh --triage "$WORK/debt-fb.triage.json" --out "$WORK/debt-fb.after.json" --entry e1
expect "a fallback unit is proven by name, file and dimension (exit 0)" "$RC" 0
contains "... and says it was matched that way" "$OUT" "matched by name, file and dimension: the unit has no string id"
contains "... denominator counts the fallback" "$ERR" "fell back to name, file and dimension 1"
STUB_REVIEW=review-sibling-first-resolved run "$B" remeasure.sh --triage "$WORK/debt-fb.triage.json" --out "$WORK/debt-fb.after2.json" --entry e1
expect "near miss: two entries of the re-run share the fallback unit's name, file and dimension -> not proven (exit 1)" "$RC" 1
contains "... the reason is the missing id" "$OUT" "carries no id to tell them apart"
review_doc "[$(offender resolved '"src/`covered.rs`/other_fn()."' other_fn src/covered.rs 50 5 null resolved)]" >"$FX/review-othername.json"
STUB_REVIEW=review-othername run "$B" remeasure.sh --triage "$WORK/debt-fb.triage.json" --out "$WORK/debt-fb.after3.json" --entry e1
expect "near miss: an offender of another name in the same file and dimension is not the fallback unit (exit 1)" "$RC" 1
contains "... not proven" "$OUT" "not proven: e1 covered_fn"
rm -f "$FX/impact.covered_fn.json"
expect "RV_MIN_LOGOS stays 1.20.0" "$(grep -c '^RV_MIN_LOGOS="1.20.0"$' "$SCR/lib/common.sh")" 1

echo "== edit-scope.sh: pre-dirty files, hashes, deletions, HEAD, the contract, a subdirectory"
R2="$WORK/repo2"
mkdir -p "$R2/pkg/src" "$R2/.logos"
git -C "$R2" init -q -b main . >/dev/null 2>&1 || { git -C "$R2" init -q . && git -C "$R2" checkout -q -b main; }
printf 'a\n' >"$R2/a.txt"; printf 'b\n' >"$R2/b.txt"; printf 's\n' >"$R2/sp ace.txt"; printf 'r\n' >"$R2/pkg/src/a.rs"; printf 'rules\n' >"$R2/.logos/rules.toml"
git -C "$R2" -c user.email=t@t -c user.name=t -c commit.gpgsign=false add -A >/dev/null
git -C "$R2" -c user.email=t@t -c user.name=t -c commit.gpgsign=false commit -q -m base
r2() { git -C "$R2" -c user.email=t@t -c user.name=t -c commit.gpgsign=false "$@"; }
r2_reset() { r2 reset -q --hard; r2 clean -fdq; }
scope() { # mode args... -> OUT/ERR/RC, project = repo2
    run "$B" edit-scope.sh "$1" --state "$WORK/r2.state" --project "$R2" "${@:2}"
}
r2_reset; printf 'dirty\n' >"$R2/a.txt"
scope begin
contains "begin records the dirty path" "$OUT" "recorded 1 dirty path(s)"
scope check
expect "a file dirty at begin and untouched since is not a change (exit 0)" "$RC" 0
printf 'dirty again\n' >"$R2/a.txt"
scope check
expect "the same file edited again IS a change: the hash differs (exit 1)" "$RC" 1
contains "... named" "$OUT" "outside the fix set: a.txt"
scope check --allow a.txt
expect "... allowed (exit 0)" "$RC" 0
scope check --allow ./a.txt
expect "... a leading ./ on --allow is dropped (exit 0)" "$RC" 0
r2 checkout -q -- a.txt
scope check
expect "a file dirty at begin and reverted since is a change too (exit 1): the user's pending work is not discarded unseen" "$RC" 1
contains "... named" "$OUT" "outside the fix set: a.txt"
r2_reset; printf 'dirty\n' >"$R2/a.txt"; scope begin
rm -f "$R2/b.txt"
scope check
expect "a deleted tracked file is a change (exit 1)" "$RC" 1
contains "... named" "$OUT" "outside the fix set: b.txt"
scope check --allow b.txt
expect "... and allowed (exit 0)" "$RC" 0
git -C "$R2" checkout -q -- b.txt
printf 'x\n' >"$R2/a.txt.bak"
scope check --allow a.txt
expect "near miss: allowing a.txt does not allow a.txt.bak (exit 1)" "$RC" 1
contains "... the near miss is named" "$OUT" "outside the fix set: a.txt.bak"
rm -f "$R2/a.txt.bak"
printf 'edit\n' >"$R2/sp ace.txt"
scope check --allow "sp ace.txt"
expect "a path with a space is allowed by its exact name (exit 0)" "$RC" 0
r2_reset; scope begin
printf 'edit\n' >"$R2/a.txt"
r2 commit -q -am sneak
scope check --allow a.txt
expect "an edit that was committed after begin: HEAD moved (exit 1), allowed or not" "$RC" 1
contains "... says HEAD moved" "$OUT" "HEAD moved"
r2 reset -q --hard HEAD~1
r2_reset; printf 'noise\n' >"$R2/.logos/db"; scope begin
printf 'more\n' >"$R2/.logos/db"
scope check
expect "the store under .logos/ is not a path (exit 0)" "$RC" 0
printf 'relaxed\n' >"$R2/.logos/rules.toml"
scope check
expect "the quality contract inside .logos/ is (exit 1)" "$RC" 1
contains "... named" "$OUT" "outside the fix set: .logos/rules.toml"
r2_reset
# a project that is a subdirectory of the repository
run "$B" edit-scope.sh begin --state "$WORK/r2.sub.state" --project "$R2/pkg"
printf 'edit\n' >"$R2/pkg/src/a.rs"; printf 'outside\n' >"$R2/b.txt"
run "$B" edit-scope.sh check --state "$WORK/r2.sub.state" --project "$R2/pkg" --allow src/a.rs
expect "in a subdirectory project paths are project-relative, and files outside it are not this project's (exit 0)" "$RC" 0
printf 'edit again\n' >"$R2/pkg/src/a.rs"
run "$B" edit-scope.sh begin --state "$WORK/r2.sub.state" --project "$R2/pkg"
printf 'third\n' >"$R2/pkg/src/a.rs"
run "$B" edit-scope.sh check --state "$WORK/r2.sub.state" --project "$R2/pkg"
expect "a file already dirty at begin, edited further, is a change by its content (not 'deleted') in a subdirectory (exit 1)" "$RC" 1
contains "... named project-relative" "$OUT" "outside the fix set: src/a.rs"
r2_reset
run "$B" edit-scope.sh check --state "$WORK/missing.state" --project "$R2"
expect "check with no state file: exit 2" "$RC" 2
run "$B" edit-scope.sh frobnicate --state "$WORK/x.state"
expect "an unknown mode: exit 2" "$RC" 2
run "$B" edit-scope.sh begin --state "$WORK/x.state" --project "$WORK/plain"
expect "not a git repository: exit 3" "$RC" 3
run "$B" edit-scope.sh begin --state "$WORK/no-such-dir/x.state" --project "$R2"
expect "a state file in a missing directory is not the 'other answer' (exit 3 or 1 from the shell, never 0)" "$([ "$RC" -ne 0 ] && echo nonzero)" nonzero

echo "== merge-base.sh: the other bases"
newrepo() { # dir branch -> a repo with one commit on branch
    mkdir -p "$1" && git -C "$1" init -q -b "$2" . >/dev/null 2>&1 || { git -C "$1" init -q . && git -C "$1" checkout -q -b "$2"; }
    printf 'x\n' >"$1/f" && git -C "$1" add -A && git -C "$1" -c user.email=t@t -c user.name=t -c commit.gpgsign=false commit -q -m c
}
newrepo "$WORK/m1" master
run "$B" merge-base.sh --project "$WORK/m1"
contains "a repository whose default branch is master: base=master" "$OUT" "base=master"
contains "... the denominator counts main as tried first" "$ERR" "2 candidate ref(s) tried, 1 resolved"
newrepo "$WORK/m2" main
git -C "$WORK/m2" update-ref refs/remotes/origin/trunk HEAD
git -C "$WORK/m2" symbolic-ref refs/remotes/origin/HEAD refs/remotes/origin/trunk
run "$B" merge-base.sh --project "$WORK/m2"
contains "origin/HEAD outranks main" "$OUT" "base=origin/trunk"
newrepo "$WORK/m3" develop
run "$B" merge-base.sh --project "$WORK/m3"
expect "no origin/HEAD, main or master: exit 1" "$RC" 1
contains "... says so and how to name a base" "$OUT" "no origin/HEAD, main or master"
contains "... three candidates were tried" "$ERR" "2 candidate ref(s) tried, 0 resolved"
SHALLOW="$WORK/m4"
git clone -q --depth 1 "file://$WORK/m2" "$SHALLOW" >/dev/null 2>&1 && {
    git -C "$SHALLOW" -c user.email=t@t -c user.name=t -c commit.gpgsign=false commit -q --allow-empty -m more >/dev/null 2>&1
    run "$B" merge-base.sh --project "$SHALLOW" --base origin/main
    ok "(a shallow clone is probed)"
}

echo "== debt-list.sh: the rest"
printf 'x\n' >"$REPO/src/thin.py"
cat >"$FX/improvements.json" <<'EOF'
{"recorded":true,"basis":"estimate","outliers":[],"units":[
 {"kind":"function","node_id":1,"name":"a","file":"src/thin.py","line":5,"dimensions":["nesting"],"points":9,"detail":"d","outliers":[]},
 {"kind":"function","node_id":2,"name":"b","file":"src/Up.PY","line":5,"dimensions":["nesting"],"points":8,"detail":"d","outliers":[]},
 {"kind":"function","node_id":3,"name":"c","file":"cmd/main.go","line":5,"dimensions":["nesting"],"points":7,"detail":"d","outliers":[]},
 {"kind":"function","node_id":4,"name":"d","file":"src/covered.rs","line":5,"dimensions":["nesting"],"points":6,"detail":"d","outliers":[]}]}
EOF
printf '{"languages":[{"name":"rust","extensions":["rs"]},{"name":"python","extensions":["py"]},{"name":"go","extensions":["go"]}]}\n' >"$FX/languages.json"
printf '{"resolution_by_language":[{"language":"rust","calls":{"cross_file_absence":null}},{"language":"python","calls":{"cross_file_absence":{"cause":"same-file-only"}}},{"language":"go","calls":{"cross_file_absence":{"cause":"no-resolved-edges"}}}]}\n' >"$FX/status.json"
run "$B" debt-list.sh --out "$WORK/debt2.json"
expect "trust: a dirty Python file is stale (not its language's cause); an upper-case extension is read as python; a go unit reads no-resolved-edges; a clean rust unit is resolved" \
    "$(jget "$WORK/debt2.json" '",".join(e["trust"] for e in d["entries"])')" "stale,same-file-only,no-resolved-edges,resolved"
git -C "$REPO" checkout -q -- src/thin.py
printf 'this is not json\n' >"$FX/improvements.json"
run "$B" debt-list.sh --out "$WORK/debt3.json"
expect "an improvements answer that is not JSON (an older logos): exit 3" "$RC" 3
expect "... and no output file" "$([ -e "$WORK/debt3.json" ] && echo kept || echo none)" none

echo "== the skill, the subagent and the command"
SKILLMD="$SKILL/SKILL.md"
frontmatter() { # file key
    python3 -I - "$1" "$2" <<'PY'
import re, sys
text = open(sys.argv[1], encoding="utf-8").read()
m = re.match(r"---\n(.*?)\n---\n", text, re.S)
if not m:
    print("")
else:
    for line in m.group(1).splitlines():
        k, _, v = line.partition(":")
        if k.strip() == sys.argv[2]:
            print(v.strip())
PY
}
expect "SKILL.md: name is logos-review" "$(frontmatter "$SKILLMD" name)" logos-review
if [ -n "$(frontmatter "$SKILLMD" description)" ]; then ok "SKILL.md: has a description"; else bad "SKILL.md: no description"; fi
for s in merge-base review-delta coverage-lookup trust-gate edit-scope remeasure check-report debt-list; do
    contains "SKILL.md names scripts/$s.sh" "$(cat "$SKILLMD")" "scripts/$s.sh"
done
# every script path SKILL.md names exists and is executable
NAMED="$(grep -o 'scripts/[a-z-]*\.sh' "$SKILLMD" | LC_ALL=C sort -u)"
MISSING=""
for n in $NAMED; do [ -x "$SKILL/$n" ] || MISSING="$MISSING $n"; done
expect "every script SKILL.md names exists and is executable" "$MISSING" ""
for v in fix test-first no-action-warranted untrusted; do
    contains "SKILL.md states the verdict $v" "$(cat "$SKILLMD")" "$v"
done
for phrase in "Never cite the aggregate quality number" "report it as \`cleared\`" "--triage \$W/triage.json" "Cross-rank it with churn" \
    "Do not commit" "the same \`--base\` the triage used" "the entry's \`symbol\` is that id" "never the unit" \
    "names no single symbol" "falls back to name,"; do
    contains "SKILL.md states: $phrase" "$(cat "$SKILLMD")" "$phrase"
done
expect "the skill names no host: no CLAUDE_ variable anywhere in it (FR-IN-15: the body and the scripts are host-neutral)" \
    "$(grep -rl 'CLAUDE_' "$SKILL" "$PLUGIN/agents/logos-reviewer.md" 2>/dev/null | grep -v '/test-' | LC_ALL=C sort | tr '\n' ' ')" ""
AGENT="$PLUGIN/agents/logos-reviewer.md"
expect "the subagent: name is logos-reviewer" "$(frontmatter "$AGENT" name)" logos-reviewer
if [ -n "$(frontmatter "$AGENT" description)" ]; then ok "the subagent: has a description"; else bad "the subagent: no description"; fi
contains "the subagent runs the skill" "$(cat "$AGENT")" "logos-review"
contains "the subagent may call the Skill tool its text tells it to use" ",$(frontmatter "$AGENT" tools | tr -d ' ')," ",Skill,"
CMD="$PLUGIN/commands/review.md"
if [ -n "$(frontmatter "$CMD" description)" ]; then ok "the command: has a description"; else bad "the command: no description"; fi
contains "the command starts the subagent by its plugin-qualified name" "$(cat "$CMD")" "logos:logos-reviewer"
contains "the command takes its arguments" "$(cat "$CMD")" '$ARGUMENTS'

echo
echo "$PASS passed, $FAIL failed (of $((PASS + FAIL)))"
[ "$FAIL" -eq 0 ] || exit 1
exit 0
