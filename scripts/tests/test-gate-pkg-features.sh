#!/usr/bin/env bash
#
# test-gate-pkg-features.sh — prove scripts/gate.sh gives `web` (and only `web`)
# `--features lang-all` on its per-package test runs (S-511, CR-167).
#
# gate.sh runs when executed, so it cannot be sourced; the functions involved are
# cut out of it by name and run here. Two layers: the flag decision itself
# (pkg_features), then the OUTPUT surfaces a human or the evidence validator reads
# — the printed `reproduce:` line, each unit's `argv`, the gate-level argv — by
# running the real run_test_unit/gate_tests against a stub `cargo`.
#
# Usage: bash scripts/tests/test-gate-pkg-features.sh
# Exit:  0 all cases behaved, 1 a case did not

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
GATE="$HERE/scripts/gate.sh"

WORK="$HERE/scripts/tests/.gate-pkg-features.tmp.$$"
trap 'rm -rf "$WORK"' EXIT INT TERM
mkdir -p "$WORK" || exit 1

extract() { # function-name -> its definition, from `name() {` to the first `^}` outside a heredoc
    awk -v n="$1" '
        $0 ~ "^"n"\\(\\) \\{" { p = 1 }
        p { print }
        p && /<<.?PY.?$/ { h = 1; next }
        p && h && /^PY$/ { h = 0; next }
        p && !h && /^}/ { exit }
    ' "$GATE"
}
FUNCS="$(extract flavor_features; extract pkg_features)"
LEGFUNCS="$FUNCS
$(for f in reset_units add_unit write_evidence run_test_unit gate_tests; do extract "$f"; done)"
if [ -z "$FUNCS" ] || ! printf '%s' "$FUNCS" | grep -q '^pkg_features()'; then
    echo "FAIL  could not extract flavor_features/pkg_features from gate.sh"
    exit 1
fi

PASS=0
FAIL=0

# check <name> <flavor> <pkg> <expected FEAT, space-joined>
check() {
    local name="$1" flavor="$2" pkg="$3" want="$4" got
    got="$(
        set -u
        FEAT=()
        eval "$FUNCS"
        pkg_features "$flavor" "$pkg"
        echo "${FEAT[*]+${FEAT[*]}}"
    )"
    if [ "$got" = "$want" ]; then
        echo "  ok    $name"
        PASS=$((PASS + 1))
    else
        echo "  FAIL  $name: got '$got', wanted '$want'"
        FAIL=$((FAIL + 1))
    fi
}

check "web, default flavor, takes lang-all"      default web        "--features lang-all"
check "web, agents flavor, keeps agents + lang-all" agents web      "--features agents --features lang-all"
check "web, noext flavor, lang-all not repeated" noext   web        "--no-default-features --features lang-all,ui"
check "logos-core, default flavor, unchanged"    default logos-core ""
check "logos-core, agents flavor, unchanged"     agents  logos-core "--features agents"
check "mcp, default flavor, unchanged"           default mcp        ""
check "logos, noext flavor, unchanged"           noext   logos      "--no-default-features --features lang-all,ui"
check "near miss 'web-extra' is not web"         default web-extra  ""
check "near miss 'webx' is not web"              default webx       ""

# An unknown flavor must still be a usage error (exit 2), not a silent default.
(
    FEAT=()
    eval "$FUNCS"
    pkg_features bogus web
) >/dev/null 2>&1
rc=$?
if [ "$rc" -eq 2 ]; then
    echo "  ok    unknown flavor exits 2"
    PASS=$((PASS + 1))
else
    echo "  FAIL  unknown flavor exited $rc, wanted 2"
    FAIL=$((FAIL + 1))
fi

# --- output surfaces: run the real legs against a stub cargo -----------------
#
# leg <gate> <flavor> <pkg>... runs gate_tests with `cargo` replaced by a function
# that reports one passing test binary, and prints the gate's stdout. The evidence
# lands in $WORK/<gate>.json.
leg() {
    local gate="$1" flavor="$2"
    shift 2
    (
        set -uo pipefail
        EVID="$WORK" TIER=fast TREE_ID=t HEAD_SHA=h RUNNER=r WORKTREE=w
        SKIPS=()
        eval "$LEGFUNCS"
        cargo() {
            echo "     Running tests/x.rs (target/debug/deps/x-1)" >&2
            echo "test result: ok. 3 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out"
        }
        pkg_field() { # name field: 1 expected binary, web has no agents feature
            case "$2" in 2) echo 1 ;; 5) [ "$1" = web ] && echo no || echo yes ;; esac
        }
        floor_for() { echo 1; }
        gate_done() { return 1; }
        record() { printf '  %s %s\n' "$1" "$2"; }
        gate_tests "$gate" "$flavor" "$@"
    )
}

# jq-free JSON probe: print <unit>'s argv, or the gate argv for unit "-"
argv_of() { # json unit
    python3 -I -c 'import json,sys
d = json.load(open(sys.argv[1]))
if sys.argv[2] == "-":
    print(d["argv"])
else:
    print(next((u["argv"] for u in d["units"] if u["unit"] == sys.argv[2]), "ABSENT"))' "$1" "$2"
}

# expect <name> <got> <contains|lacks> <needle>
expect() {
    local name="$1" got="$2" mode="$3" needle="$4" hit=no
    case "$got" in *"$needle"*) hit=yes ;; esac
    if { [ "$mode" = contains ] && [ "$hit" = yes ]; } || { [ "$mode" = lacks ] && [ "$hit" = no ]; }; then
        echo "  ok    $name"
        PASS=$((PASS + 1))
    else
        echo "  FAIL  $name: '$got' should $mode '$needle'"
        FAIL=$((FAIL + 1))
    fi
}

OUT="$(leg test default web logos-core)"
expect "default leg prints web's reproduce command" "$OUT" contains "reproduce: cargo test -p web --jobs 2 --no-fail-fast --features lang-all -- "
expect "default leg prints exactly one reproduce line" "$(printf '%s\n' "$OUT" | grep -c 'reproduce:')" contains "1"
expect "default leg prints none for logos-core" "$OUT" lacks "reproduce: cargo test -p logos-core"
expect "web unit evidence argv names the flag" "$(argv_of "$WORK/test.json" web)" contains "--features lang-all"
expect "logos-core unit evidence argv lacks the flag" "$(argv_of "$WORK/test.json" logos-core)" lacks "lang-all"
expect "logos-core unit evidence argv is a real command" "$(argv_of "$WORK/test.json" logos-core)" contains "cargo test -p logos-core --jobs 2"
expect "gate-level argv names web's flag" "$(argv_of "$WORK/test.json" -)" contains "; web: --features lang-all"

OUT="$(leg test-agents agents web logos-core)"
expect "agents leg skips web (no agents feature)" "$(argv_of "$WORK/test-agents.json" web)" contains "ABSENT"
expect "agents leg prints no reproduce line" "$OUT" lacks "reproduce:"
expect "agents leg keeps logos-core's agents flag" "$(argv_of "$WORK/test-agents.json" logos-core)" contains "--features agents"
expect "agents leg gate-level argv has no web note" "$(argv_of "$WORK/test-agents.json" -)" lacks "web:"

# add_unit must tolerate the 11-argument form (a unit with no argv): an unbound
# ${12} under `set -u` aborts the whole gate (the playwright unit once did).
OUT="$(
    set -uo pipefail
    eval "$LEGFUNCS"
    UNITS=""
    add_unit u 1 1 1 1 1 0 0 0 pass none
    echo survived
)"
expect "add_unit tolerates a call without the argv field" "$OUT" contains "survived"

echo "$PASS passed, $FAIL failed (of $((PASS + FAIL)))"
[ "$FAIL" -eq 0 ]
