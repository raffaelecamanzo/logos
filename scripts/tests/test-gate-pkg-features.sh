#!/usr/bin/env bash
#
# test-gate-pkg-features.sh — prove scripts/gate.sh gives `--features lang-all` to
# every package that declares it and does not enable it by default, chosen by a
# rule over `cargo metadata` and never by package name (S-718, CR-227; the by-name
# `web` rule it replaces was S-511, CR-167).
#
# gate.sh runs when executed, so it cannot be sourced; the functions involved are
# cut out of it by name and run here. Three layers:
#   1. the rule (build_pkg_table) over FIXTURE metadata — off by default, default
#      includes it (directly and through another feature), no such feature, near
#      misses, and a package called `web` whose default already includes it;
#   2. the flag decision (pkg_features) on top of that table;
#   3. the OUTPUT surfaces a human or the evidence validator reads — the printed
#      `reproduce:` line, each unit's `argv`, the gate-level argv — by running the
#      real run_test_unit/gate_tests against a stub `cargo`.
# Then a mutation phase proves each case can fail: a one-line change to a copy of
# gate.sh must turn the suite red, on the case it targets.
#
# Usage: bash scripts/tests/test-gate-pkg-features.sh
# Env:   GATE_SH  path of the gate script under test (default: scripts/gate.sh)
# Exit:  0 all cases behaved, 1 a case did not

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
GATE="${GATE_SH:-$HERE/scripts/gate.sh}"
SELF="$HERE/scripts/tests/test-gate-pkg-features.sh"

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
FUNCS="$(for f in flavor_features pkg_features pkg_field build_pkg_table; do extract "$f"; done)"
LEGFUNCS="$FUNCS
$(for f in reset_units add_unit write_evidence run_test_unit gate_tests; do extract "$f"; done)"
for f in flavor_features pkg_features pkg_field build_pkg_table; do
    if ! printf '%s' "$FUNCS" | grep -q "^$f()"; then
        echo "FAIL  could not extract $f from $GATE"
        exit 1
    fi
done

PASS=0
FAIL=0

ok() { echo "  ok    $1"; PASS=$((PASS + 1)); }
bad() { echo "  FAIL  $1"; FAIL=$((FAIL + 1)); }

# --- fixture metadata ---------------------------------------------------------
#
# One package per shape. Names are deliberately NOT the workspace's: `dash` is the
# web-like package (lang-all forwarded, off by default), `engine` the core-like one
# (lang-all declared, off by default), and the package called `web` is the one whose
# default already enables the feature — a by-name rule would flag it.
FIXTURE="$WORK/metadata.json"
python3 -I - "$FIXTURE" <<'PY'
import json, sys

def pkg(name, features):
    return {
        "name": name,
        "features": features,
        "targets": [{"kind": ["lib"], "test": True, "doctest": False}],
        "manifest_path": "/fixture/%s/Cargo.toml" % name,
    }

packages = [
    # off by default -> flag
    pkg("engine", {"default": ["lang-rust"], "lang-rust": [],
                   "lang-all": ["lang-rust"]}),
    pkg("dash", {"default": [], "lang-all": ["engine/lang-all"]}),
    # default names a DEPENDENCY's lang-all, not its own -> still off -> flag
    # (`dep:x` and the weak `x?/lang-all` form name no feature of this package either)
    pkg("borrower", {"default": ["engine/lang-all", "dep:foo", "dash?/lang-all"],
                     "lang-all": []}),
    # no `default` key at all (cargo omits it when undeclared) -> off -> flag
    pkg("nodefault", {"lang-all": []}),
    # default includes it -> no flag
    pkg("app", {"default": ["lang-all", "ui"], "ui": [], "lang-all": []}),
    pkg("web", {"default": ["lang-all"], "lang-all": []}),
    pkg("viaui", {"default": ["full"], "full": ["lang-all"], "lang-all": []}),
    pkg("threehop", {"default": ["a"], "a": ["b"], "b": ["lang-all"], "lang-all": []}),
    # no such feature -> no flag
    pkg("plain", {"default": []}),
    pkg("nofeat", {}),
    pkg("nearmiss", {"default": [], "lang-all-extra": [], "lang-allx": []}),
]
with open(sys.argv[1], "w") as fh:
    json.dump({"packages": packages}, fh)
PY
(
    eval "$FUNCS"
    build_pkg_table "$FIXTURE"
) >"$WORK/table.tsv" 2>"$WORK/table.err"
if [ ! -s "$WORK/table.tsv" ]; then
    bad "[table] build_pkg_table produced no rows from the fixture: $(cat "$WORK/table.err")"
    echo "$PASS passed, $FAIL failed (of $((PASS + FAIL)))"
    exit 1
fi
TABLE="$(cat "$WORK/table.tsv")"

# table <name> <pkg> <expected field 5>
table() {
    local name="$1" pkg="$2" want="$3" got
    got="$(printf '%s\n' "$TABLE" | awk -F'\t' -v p="$pkg" '$1 == p { print $5 }')"
    if [ "$got" = "$want" ]; then ok "$name"; else bad "$name: field 5 of '$pkg' is '$got', wanted '$want'"; fi
}

table "[off-by-default] core-like package is selected"            engine   yes
table "[off-by-default] forwarding-only package is selected"      dash     yes
table "[off-by-default] default naming a dependency's lang-all is not its own" borrower yes
table "[off-by-default] a package with no default feature at all is selected" nodefault yes
table "[default-includes] direct default entry is not selected"   app      no
table "[default-includes] a package called web with it on default is not selected" web no
table "[default-includes] transitive default entry is not selected" viaui  no
table "[default-includes] a three-hop default chain is not selected" threehop no
table "[no-such-feature] empty default is not selected"           plain    no
table "[no-such-feature] no features at all is not selected"      nofeat   no
table "[no-such-feature] near-miss feature names are not selected" nearmiss no

# check <name> <flavor> <pkg> <expected FEAT, space-joined>
check() {
    local name="$1" flavor="$2" pkg="$3" want="$4" got
    got="$(
        set -u
        FEAT=()
        PKG_TABLE="$TABLE"
        eval "$FUNCS"
        pkg_features "$flavor" "$pkg"
        echo "${FEAT[*]+${FEAT[*]}}"
    )"
    if [ "$got" = "$want" ]; then ok "$name"; else bad "$name: got '$got', wanted '$want'"; fi
}

check "[off-by-default] engine, default flavor, takes lang-all"  default engine   "--features lang-all"
check "[off-by-default] dash, default flavor, takes lang-all"    default dash     "--features lang-all"
check "[off-by-default] borrower, default flavor, takes lang-all" default borrower "--features lang-all"
check "[off-by-default] nodefault, default flavor, takes lang-all" default nodefault "--features lang-all"
check "[default-includes] app, default flavor, unchanged"        default app      ""
check "[default-includes] web by name is not special"            default web      ""
check "[default-includes] viaui, default flavor, unchanged"      default viaui    ""
check "[default-includes] threehop, default flavor, unchanged"   default threehop ""
check "[no-such-feature] plain, default flavor, unchanged"       default plain    ""
check "[no-such-feature] nearmiss, default flavor, unchanged"    default nearmiss ""
check "[no-such-feature] a package absent from the table is unchanged" default ghost ""

# An unknown flavor must still be a usage error (exit 2), not a silent default.
(
    FEAT=()
    PKG_TABLE="$TABLE"
    eval "$FUNCS"
    pkg_features bogus engine
) >/dev/null 2>&1
rc=$?
if [ "$rc" -eq 2 ]; then ok "unknown flavor exits 2"; else bad "unknown flavor exited $rc, wanted 2"; fi

# The build this repository ships composes no extension, so `default` is the only
# flavor: the two the gate once ran beside it are usage errors, like any other name.
for retired in agents noext; do
    (
        FEAT=()
        PKG_TABLE="$TABLE"
        eval "$FUNCS"
        pkg_features "$retired" engine
    ) >/dev/null 2>&1
    rc=$?
    if [ "$rc" -eq 2 ]; then ok "[flavors] the retired $retired flavor exits 2"; else bad "[flavors] the retired $retired flavor exited $rc, wanted 2"; fi
done

# --- the rule against the real workspace --------------------------------------
#
# The fixtures prove the rule; this proves the repository still has the shape the
# rule was written for. Today it selects logos-core and web — and not cli (`logos`)
# or mcp, whose defaults enable the feature. A change to the selection is a
# decision, so it must be a red here first.
if cargo metadata --manifest-path "$HERE/Cargo.toml" --no-deps --format-version 1 >"$WORK/real.json" 2>"$WORK/real.err"; then
    REAL="$(
        eval "$FUNCS"
        build_pkg_table "$WORK/real.json" | awk -F'\t' '$5 == "yes" { print $1 }' | tr '\n' ' '
    )"
    if [ "$REAL" = "logos-core web " ]; then
        ok "[real workspace] the rule selects exactly logos-core and web"
    else
        bad "[real workspace] the rule selects '$REAL', wanted 'logos-core web '"
    fi
else
    bad "[real workspace] cargo metadata failed: $(head -n 3 "$WORK/real.err")"
fi

# --- output surfaces: run the real legs against a stub cargo -----------------
#
# leg <gate> <flavor> <pkg>... runs gate_tests with `cargo` replaced by a function
# that reports one passing test binary, and prints the gate's stdout. The evidence
# lands in $WORK/<gate>.json. pkg_field is the real one, over the fixture table.
leg() {
    local gate="$1" flavor="$2"
    shift 2
    : >"$WORK/cargo-$gate.calls"
    (
        set -uo pipefail
        EVID="$WORK" TIER=fast TREE_ID=t HEAD_SHA=h RUNNER=r WORKTREE=w
        SKIPS=()
        PKG_TABLE="$TABLE"
        eval "$LEGFUNCS"
        cargo() {
            echo "$*" >>"$WORK/cargo-$gate.calls"
            echo "     Running tests/x.rs (target/debug/deps/x-1)" >&2
            echo "test result: ok. 3 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out"
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

# expect <name> <got> <contains|lacks|equals> <needle>
expect() {
    local name="$1" got="$2" mode="$3" needle="$4" hit=no
    case "$got" in *"$needle"*) hit=yes ;; esac
    if { [ "$mode" = contains ] && [ "$hit" = yes ]; } || { [ "$mode" = lacks ] && [ "$hit" = no ]; } ||
        { [ "$mode" = equals ] && [ "$got" = "$needle" ]; }; then
        ok "$name"
    else
        bad "$name: '$got' should $mode '$needle'"
    fi
}

# called_as_recorded <gate> <pkg...>: the command the leg RAN (the stub records its
# arguments) must be exactly the `argv` the evidence records for that unit — the
# flag has to reach cargo, not only the text that describes the run.
called_as_recorded() {
    local gate="$1" pkg ran recorded
    shift
    for pkg in "$@"; do
        ran="cargo $(grep -F "test -p $pkg " "$WORK/cargo-$gate.calls")"
        recorded="$(argv_of "$WORK/$gate.json" "$pkg" | sed 's/[[:space:]]*$//')"
        expect "[surfaces] $gate ran $pkg exactly as its evidence argv records" "$ran" equals "$recorded"
    done
}

OUT="$(leg test default engine dash app plain)"
called_as_recorded test engine dash app plain
expect "[surfaces] cargo received engine's flag" "$(grep -F 'test -p engine ' "$WORK/cargo-test.calls")" contains "--features lang-all"
expect "[surfaces] cargo received no flag for app" "$(grep -F 'test -p app ' "$WORK/cargo-test.calls")" lacks "lang-all"
expect "[surfaces] gate-level argv template is the flavor's flags plus per-package notes" "$(argv_of "$WORK/test.json" -)" equals "cargo test -p <pkg> --jobs 2 --no-fail-fast -- --test-threads=2 ; engine: --features lang-all; dash: --features lang-all"
expect "[surfaces] test leg prints engine's reproduce command" "$OUT" contains "reproduce: cargo test -p engine --jobs 2 --no-fail-fast --features lang-all -- "
expect "[surfaces] test leg prints dash's reproduce command" "$OUT" contains "reproduce: cargo test -p dash --jobs 2 --no-fail-fast --features lang-all -- "
expect "[surfaces] test leg prints exactly the selected packages' reproduce lines" "$(printf '%s\n' "$OUT" | grep -c 'reproduce:')" contains "2"
expect "[surfaces] test leg prints none for app" "$OUT" lacks "reproduce: cargo test -p app"
expect "[surfaces] test leg prints none for plain" "$OUT" lacks "reproduce: cargo test -p plain"
expect "[surfaces] engine unit evidence argv names the flag" "$(argv_of "$WORK/test.json" engine)" contains "--features lang-all"
expect "[surfaces] dash unit evidence argv names the flag" "$(argv_of "$WORK/test.json" dash)" contains "--features lang-all"
expect "[surfaces] app unit evidence argv lacks the flag" "$(argv_of "$WORK/test.json" app)" lacks "lang-all"
expect "[surfaces] plain unit evidence argv lacks the flag" "$(argv_of "$WORK/test.json" plain)" lacks "lang-all"
expect "[surfaces] app unit evidence argv is a real command" "$(argv_of "$WORK/test.json" app)" contains "cargo test -p app --jobs 2"
expect "[surfaces] gate-level argv names engine's flag" "$(argv_of "$WORK/test.json" -)" contains "; engine: --features lang-all"
expect "[surfaces] gate-level argv names dash's flag" "$(argv_of "$WORK/test.json" -)" contains "; dash: --features lang-all"
expect "[surfaces] gate-level argv names no unselected package" "$(argv_of "$WORK/test.json" -)" lacks "; app:"

# A leg given no package runs no unit, so the flavor's flags come from the reset
# after the loop alone — which is also what refuses an unknown flavor there, rather
# than writing evidence for a build nobody named.
leg test-empty bogus >/dev/null 2>&1
rc=$?
expect "[surfaces] a leg with no package still refuses an unknown flavor (exit $rc)" "$rc" equals "2"
expect "[surfaces] a refused leg writes no evidence" "$([ -e "$WORK/test-empty.json" ] && echo written || echo none)" equals "none"

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

# --- mutation phase ----------------------------------------------------------
#
# Each mutation is ONE line of gate.sh changed in a copy, and the suite re-run
# against that copy with GATE_SH. It must exit non-zero AND print a FAIL line
# carrying the tag of the case it targets — a mutation that turns something else
# red proves nothing about its own case. A mutation whose target text is not in
# gate.sh (the script was reshaped) is itself a failure: a silent no-op mutation
# would read as a case that cannot fail.
NL=$'\n'
MUTATIONS=(
    "table|feats.get(f, [])|feats[f]"
    "default-includes|if f in seen:|if f in seen or len(seen) >= 3:"
    "surfaces|--no-fail-fast \${feat[@]+\"\${feat[@]}\"} -- \\|--no-fail-fast -- \\"
    "surfaces|last selected package's.${NL}    flavor_features \"\$flavor\"|last selected package's."
    "off-by-default|[ \"\$(pkg_field \"\$2\" 5)\" = yes ]|[ \"\$(pkg_field \"\$2\" 5)\" = never ]"
    "default-includes|seen, todo = set(), [\"default\"]|seen, todo = set(), []"
    "default-includes|if x in feats)|if x in feats and f == \"default\")"
    "no-such-feature|lang_all_off = \"lang-all\" in feats and |lang_all_off = "
    "no-such-feature|\"lang-all\" in feats and \"lang-all\" not in|any(k.startswith(\"lang-all\") for k in feats) and \"lang-all\" not in"
    "default-includes|[ \"\$(pkg_field \"\$2\" 5)\" = yes ]|[ \"\$(pkg_field \"\$2\" 5)\" = yes ] || [ \"\$2\" = web ]"
    "flavors|        default) FEAT=() ;;|        default) FEAT=() ;;${NL}        agents) FEAT=(--features agents) ;;"
    "surfaces|pkgnotes=\"\$pkgnotes; \$pkg: \${FEAT[*]}\"|pkgnotes=\"\$pkgnotes\""
    "surfaces|printf '      reproduce: %s\\n' \"\$argv\"|:"
)

mutate() { # tag old new
    local tag="$1" old="$2" new="$3" copy="$WORK/gate-mutant.sh" out rc
    if ! python3 -I - "$GATE" "$copy" "$old" "$new" <<'PY'
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
    out="$(GATE_SH="$copy" GATE_PKG_FEATURES_INNER=1 bash "$SELF" 2>&1)"
    rc=$?
    if [ "$rc" -ne 0 ] && printf '%s\n' "$out" | grep -q "FAIL  \[$tag\]"; then
        ok "[mutation:$tag] '$old' turns the suite red on its case"
    else
        bad "[mutation:$tag] '$old' left the suite green on its case (exit $rc)"
    fi
}

if [ -z "${GATE_PKG_FEATURES_INNER:-}" ]; then
    for m in "${MUTATIONS[@]}"; do
        # Split on the FIRST two `|` only: the new text may itself hold a shell `||`.
        tag="${m%%|*}"
        rest="${m#*|}"
        old="${rest%%|*}"
        new="${rest#*|}"
        mutate "$tag" "$old" "$new"
    done
fi

echo "$PASS passed, $FAIL failed (of $((PASS + FAIL)))"
[ "$FAIL" -eq 0 ]
