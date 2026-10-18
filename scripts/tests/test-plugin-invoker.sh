#!/usr/bin/env bash
#
# test-plugin-invoker.sh — prove every logos call the plugin's hooks and skill
# scripts make carries their invoker (S-683, FR-OB-15), and that nothing else in
# the plugin declares one.
#
# The binary half (telemetry.db v5, the stamp, the `stats --json` cross-tab) is
# tested in Rust. This suite tests the half a shell owns: that each entry point
# exports LOGOS_INVOKER before it runs logos, so the binary has something to read.
#
#   1. the roster: the five entry points declare exactly their five names, once
#      each, and no other plugin script declares one — the MCP launcher above all,
#      whose server's calls are the agent's own;
#   2. a stub logos records the LOGOS_INVOKER of every call: the Stop hook's calls
#      carry hook:stop, the session-start hook's hook:session-start, a
#      logos-review script's skill:logos-review, and a logos-sec-review script's
#      skill:logos-sec-review — including the logos-review script it runs. A hook
#      run under a stray inherited value still declares its own;
#   3. with LOGOS_INVOKER_E2E_BIN set to a built logos binary, the same calls go to
#      the real binary on a fixture, and `logos stats --json` shows hook:stop,
#      hook:session-start and skill:logos-review cells beside the direct call's
#      null one.
#
# Usage: bash scripts/tests/test-plugin-invoker.sh
# Exit:  0 all cases behaved, 1 a case did not

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PLUGIN="$HERE/plugins/logos"
REVIEW_SCR="$PLUGIN/skills/logos-review/scripts"
SEC_SCR="$PLUGIN/skills/logos-sec-review/scripts"

WORK="${TMPDIR:-/tmp}/logos-plugin-invoker-test.$$"
trap 'rm -rf "$WORK"' EXIT INT TERM
mkdir -p "$WORK" || exit 1
WORK="$(cd "$WORK" && pwd -P)"
export GIT_CEILING_DIRECTORIES="$WORK"

PASS=0
FAIL=0
ok() { echo "  ok    $1"; PASS=$((PASS + 1)); }
bad() { echo "  FAIL  $1"; FAIL=$((FAIL + 1)); }
expect() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1: got '$2', wanted '$3'"; fi; }

for f in "$PLUGIN/hooks/stop.sh" "$PLUGIN/hooks/guard.sh" "$PLUGIN/hooks/session-start.sh" \
    "$REVIEW_SCR/lib/common.sh" "$SEC_SCR/lib/secrev.sh" "$REVIEW_SCR/review-delta.sh" "$SEC_SCR/sec-delta.sh"; do
    [ -f "$f" ] || { echo "FAIL  missing $f"; echo "0 passed, 1 failed (of 1)"; exit 1; }
done

GITDIR="$(cd "$(dirname "$(command -v git)")" && pwd -P)"
GIT=(git -c user.name=t -c user.email=t@t -c maintenance.auto=false -c gc.auto=0 -c commit.gpgsign=false)

# --- 1. the roster ----------------------------------------------------------------

echo "1. the roster"

# declared FILE — the value of the one `export LOGOS_INVOKER=` line in FILE, the
# `${LOGOS_INVOKER:-…}` default unwrapped; "<n lines>" when there is not exactly one.
declared() {
    local lines n
    lines="$(grep -E '^[[:space:]]*export LOGOS_INVOKER=' "$1")"
    n="$(printf '%s' "$lines" | grep -c . || true)"
    [ "$n" = 1 ] || { echo "<$n lines>"; return; }
    lines="${lines#*LOGOS_INVOKER=}"
    lines="${lines//\"/}"
    lines="${lines#\$\{LOGOS_INVOKER:-}"
    printf '%s\n' "${lines%\}}"
}
expect "stop.sh declares hook:stop" "$(declared "$PLUGIN/hooks/stop.sh")" "hook:stop"
expect "guard.sh declares hook:pre-tool-use" "$(declared "$PLUGIN/hooks/guard.sh")" "hook:pre-tool-use"
expect "session-start.sh declares hook:session-start" "$(declared "$PLUGIN/hooks/session-start.sh")" "hook:session-start"
expect "logos-review's lib declares skill:logos-review" "$(declared "$REVIEW_SCR/lib/common.sh")" "skill:logos-review"
expect "logos-sec-review's lib declares skill:logos-sec-review" "$(declared "$SEC_SCR/lib/secrev.sh")" "skill:logos-sec-review"

# Nothing else names the variable: the MCP launcher's server makes the agent's own
# calls, and a script that declared for another would mislabel them.
OTHERS=""
while IFS= read -r f; do
    case "$f" in
        "$PLUGIN/hooks/stop.sh" | "$PLUGIN/hooks/guard.sh" | "$PLUGIN/hooks/session-start.sh" | \
            "$REVIEW_SCR/lib/common.sh" | "$SEC_SCR/lib/secrev.sh") ;;
        *) OTHERS="$OTHERS ${f#"$PLUGIN"/}" ;;
    esac
done < <(grep -rlE 'LOGOS_INVOKER=' "$PLUGIN" --include='*.sh' --include='*.py' --include='*.json' || true)
expect "no other plugin script declares an invoker" "${OTHERS:-none}" "none"

# Every script of each skill reaches its lib, so the one export covers it.
MISSING=""
for s in "$REVIEW_SCR"/*.sh; do grep -q 'lib/common.sh' "$s" || MISSING="$MISSING ${s##*/}"; done
for s in "$SEC_SCR"/*.sh; do grep -q 'lib/secrev.sh' "$s" || MISSING="$MISSING ${s##*/}"; done
expect "every skill script sources the lib that declares its invoker" "${MISSING:-none}" "none"

# The secrev.sh declaration precedes the sibling lib it sources, or the sibling's
# default would win.
EXP_AT="$(grep -n 'export LOGOS_INVOKER=' "$SEC_SCR/lib/secrev.sh" | cut -d: -f1)"
SRC_AT="$(grep -n 'SR_REVIEW_SCRIPTS/lib/common.sh"$' "$SEC_SCR/lib/secrev.sh" | grep '^[0-9]*:\.' | cut -d: -f1)"
if [ -n "$EXP_AT" ] && [ -n "$SRC_AT" ] && [ "$EXP_AT" -lt "$SRC_AT" ]; then
    ok "secrev.sh declares before it sources logos-review's lib"
else
    bad "secrev.sh declares at line '${EXP_AT}', sources at '${SRC_AT}'"
fi

# --- 2. a stub logos records each call's invoker ------------------------------------

echo "2. the invoker on every call (stub logos)"

BIN="$WORK/bin"
mkdir -p "$BIN"
# Records `<first argument>|<LOGOS_INVOKER, or <unset>>` per call, and answers
# enough of each subcommand for the caller to go on.
cat >"$BIN/logos" <<'STUB'
#!/bin/sh
printf '%s|%s\n' "${1:-}" "${LOGOS_INVOKER-<unset>}" >>"$STUB_CALLS"
case "${1:-}" in
    --version) echo "logos 99.0.0" ;;
    check) printf '{"passed":true,"rules_evaluated":1,"violations":[]}\n' ;;
    evolution) printf '{"snapshots":[]}\n' ;;
    review) printf '{"base":"main","base_origin":"default-branch","head":"h","merge_base":"m","measured":true,"not_measured_because":null,"before_state":"shadow-index","offenders":[],"offender_counts":{"new":0,"worsened":0,"improved":0,"resolved":0,"unchanged":0},"violations":[],"violation_counts":{"new":0,"worsened":0,"improved":0,"resolved":0,"unchanged":0},"introduced_clone_groups":[],"stale_files":[],"sections":{},"notes":[]}\n' ;;
    --json) printf '{"systemMessage":"readout"}\n' ;;
    *) printf '{}\n' ;;
esac
exit 0
STUB
chmod +x "$BIN/logos"

N=0
# mkrepo — REPO: a git repository on a branch off main, enrolled, with a turn's
# source edit in the working tree.
mkrepo() {
    N=$((N + 1))
    REPO="$WORK/repo$N"
    mkdir -p "$REPO/.logos" "$REPO/src"
    printf 'fn a() {}\n' >"$REPO/src/lib.rs"
    (cd "$REPO" && "${GIT[@]}" init -q -b main && "${GIT[@]}" add src && "${GIT[@]}" commit -qm base &&
        "${GIT[@]}" checkout -qb feature) >/dev/null 2>&1
    printf 'fn a() {}\nfn b() {}\n' >"$REPO/src/lib.rs"
}

# invokers FILE — the distinct invokers recorded, sorted, space-joined.
invokers() { cut -d'|' -f2 "$1" 2>/dev/null | sort -u | tr '\n' ' ' | sed 's/ $//'; }
calls() { grep -c . "$1" 2>/dev/null || echo 0; }

# hook NAME [VAR=VALUE…] — run hooks/NAME as the host does (`env -i`, a payload on
# stdin, the project named), recording into $REPO.calls.
hook() {
    local name="$1"
    shift
    printf '{"hook_event_name":"x","stop_hook_active":false}' |
        env -i HOME="$WORK" PATH="$BIN:$GITDIR:/usr/bin:/bin" CLAUDE_PROJECT_DIR="$REPO" \
            STUB_CALLS="$REPO.calls" "$@" "$PLUGIN/hooks/$name" >/dev/null 2>&1 || true
}

# wait_for FILE WORD — the Stop hook may start `review` detached; wait for it.
wait_for() {
    local i=0
    while [ "$i" -lt 50 ] && ! grep -q "^$2|" "$1" 2>/dev/null; do
        sleep 0.1
        i=$((i + 1))
    done
}

mkrepo
hook stop.sh
wait_for "$REPO.calls" review
[ "$(calls "$REPO.calls")" -gt 0 ] && ok "the Stop hook ran logos ($(calls "$REPO.calls") calls)" ||
    bad "the Stop hook ran no logos, so its stamp was not exercised"
expect "every Stop-hook call carries hook:stop" "$(invokers "$REPO.calls")" "hook:stop"

mkrepo
hook stop.sh LOGOS_INVOKER=skill:stray
wait_for "$REPO.calls" review
expect "the Stop hook declares its own over a stray inherited value" "$(invokers "$REPO.calls")" "hook:stop"

mkrepo
hook session-start.sh
grep -q '^--json|' "$REPO.calls" && ok "the session-start hook ran the readout" ||
    bad "the session-start hook did not run the readout"
expect "every session-start call carries hook:session-start" "$(invokers "$REPO.calls")" "hook:session-start"

# script DIR NAME [VAR=VALUE…] -- ARGS… — run a skill script under `env -i`.
script() {
    local dir="$1" name="$2"
    shift 2
    local envs=()
    while [ $# -gt 0 ] && [ "$1" != -- ]; do envs+=("$1"); shift; done
    shift
    (cd "$REPO" && env -i HOME="$WORK" PATH="$BIN:$GITDIR:/usr/bin:/bin" LOGOS_PROJECT_DIR="$REPO" \
        STUB_CALLS="$REPO.calls" "${envs[@]+"${envs[@]}"}" bash "$dir/$name" "$@") >/dev/null 2>&1 || true
}

mkrepo
script "$REVIEW_SCR" review-delta.sh -- --out "$WORK/delta$N.json"
grep -q '^review|' "$REPO.calls" && ok "review-delta.sh ran logos review" || bad "review-delta.sh ran no review"
expect "every logos-review script call carries skill:logos-review" "$(invokers "$REPO.calls")" "skill:logos-review"

mkrepo
script "$SEC_SCR" sec-delta.sh -- --out "$WORK/sec$N.json"
grep -q '^review|' "$REPO.calls" && ok "sec-delta.sh ran logos review through review-delta.sh" ||
    bad "sec-delta.sh ran no review"
expect "a logos-review script run by logos-sec-review carries skill:logos-sec-review" \
    "$(invokers "$REPO.calls")" "skill:logos-sec-review"

# Near miss: the outer value is kept only because it was set — an empty one is not
# a declaration, and the skill's own default applies.
mkrepo
script "$REVIEW_SCR" review-delta.sh LOGOS_INVOKER= -- --out "$WORK/delta$N.json"
expect "an empty inherited value gives way to the skill's own" "$(invokers "$REPO.calls")" "skill:logos-review"

# --- 3. the real binary (opt-in) ----------------------------------------------------

if [ -n "${LOGOS_INVOKER_E2E_BIN:-}" ]; then
    echo "3. the real binary: $LOGOS_INVOKER_E2E_BIN"
    E2E="$WORK/e2e"
    mkdir -p "$E2E/bin"
    # The plugin and the skill need 1.20.0; a dev build may report less. Only
    # `--version` is answered here; every other call is the real binary's.
    cat >"$E2E/bin/logos" <<EOF
#!/bin/sh
[ "\${1:-}" = --version ] && { echo "logos 99.0.0"; exit 0; }
exec "$LOGOS_INVOKER_E2E_BIN" "\$@"
EOF
    chmod +x "$E2E/bin/logos"
    BIN="$E2E/bin"
    mkrepo
    (cd "$REPO" && env -u LOGOS_INVOKER PATH="$BIN:$GITDIR:/usr/bin:/bin" HOME="$WORK" logos index >/dev/null 2>&1) ||
        bad "the fixture did not index"
    hook session-start.sh
    hook stop.sh
    script "$REVIEW_SCR" review-delta.sh -- --out "$WORK/e2e-delta.json"
    (cd "$REPO" && env -u LOGOS_INVOKER PATH="$BIN:$GITDIR:/usr/bin:/bin" HOME="$WORK" logos languages --json >/dev/null 2>&1)
    sleep 1 # a detached `review` the Stop hook started may still be flushing
    STATS="$(cd "$REPO" && env -u LOGOS_INVOKER PATH="$BIN:$GITDIR:/usr/bin:/bin" HOME="$WORK" logos stats --json 2>/dev/null)"
    CELLS="$(printf '%s' "$STATS" | python3 -I -c '
import json, sys
d = json.load(sys.stdin)
print(" ".join(sorted({c["invoker"] or "null" for c in d["calls_by_tool_invoker"]})))')"
    for want in hook:session-start hook:stop skill:logos-review null; do
        case " $CELLS " in
            *" $want "*) ok "stats --json carries a $want cell" ;;
            *) bad "stats --json has no $want cell: [$CELLS]" ;;
        esac
    done
    COVER="$(printf '%s' "$STATS" | python3 -I -c '
import json, sys
print(json.load(sys.stdin)["attribution_coverage"]["null_invoker_includes_pre_stamp_rows"])')"
    expect "stats --json states the null-invoker limit" "$COVER" "True"
else
    echo "3. the real binary: skipped (set LOGOS_INVOKER_E2E_BIN to a built logos)"
fi

TOTAL=$((PASS + FAIL))
echo "$PASS passed, $FAIL failed (of $TOTAL)"
[ "$FAIL" -eq 0 ]
