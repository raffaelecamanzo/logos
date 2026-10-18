#!/usr/bin/env bash
#
# uat-in-06.sh — run UAT-IN-06 (the agent loop, CR-218) end to end on a fixture,
# with the agent's part played by this script (S-683).
#
# Not a gate suite: it is not named test-*.sh, because it needs a built logos,
# cargo, python3 and git, and (step 1 only) the claude CLI. Run it by hand:
#
#   LOGOS_BIN=target/debug/logos bash scripts/tests/uat-in-06.sh
#   UAT_CLAUDE=1 LOGOS_BIN=… bash scripts/tests/uat-in-06.sh   # step 1 too
#
# The fixture is a Rust crate on a feature branch off main, enrolled with
# `logos init --hooks`, a recorded snapshot on main, `max_nesting_depth = 4` in
# .logos/rules.toml, and the plugin enabled at project scope. Its calls cross files
# (`mod util`) and its tests sit in a test-marked `src/tests.rs`, so the graph binds
# Rust calls across files: with every Rust call unbound the delta's trust label
# reads `no-resolved-edges` and the skill, rightly, edits nothing. Each test step
# of the UAT is a section below; the agent's edits (the 6-deep function, its
# refactor, the rules.toml edit) are made here, and everything a hook or a skill
# would run is run as the host runs it: the hooks from plugins/logos/hooks with a
# JSON payload on stdin, the skill's scripts from its scripts/ directory.
#
# Step 1 needs a Claude Code host. With UAT_CLAUDE=1 it installs the plugin from
# this repository's marketplace into the fixture under a scratch CLAUDE_CONFIG_DIR
# (never ~/.claude) and runs one headless session; that session is not
# authenticated, but its init event (the tool roster) and its hook events arrive,
# and they are what step 1 asserts. Without UAT_CLAUDE=1 step 1 is reported as not
# run, and the plugin is enabled by writing the settings `claude plugin install`
# writes.
#
# A dev build may report a version below the plugin's minimum (1.20.0); a wrapper
# answers `--version` as 1.20.0 and passes every other call to LOGOS_BIN.
#
# Exit: 0 every step met its postconditions · 1 one did not · 3 a prerequisite is missing

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PLUGIN="$HERE/plugins/logos"
RS="$PLUGIN/skills/logos-review/scripts"

[ -n "${LOGOS_BIN:-}" ] && [ -x "$LOGOS_BIN" ] || { echo "set LOGOS_BIN to a built logos binary" >&2; exit 3; }
LOGOS_BIN="$(cd "$(dirname "$LOGOS_BIN")" && pwd -P)/$(basename "$LOGOS_BIN")"
for t in git cargo python3; do command -v "$t" >/dev/null 2>&1 || { echo "$t is not on PATH" >&2; exit 3; }; done

W="${UAT_WORK:-${TMPDIR:-/tmp}/logos-uat-in-06.$$}"
rm -rf "$W" && mkdir -p "$W" || exit 3
W="$(cd "$W" && pwd -P)"
export GIT_CEILING_DIRECTORIES="$W"
unset LOGOS_INVOKER CLAUDE_PROJECT_DIR CLAUDE_PLUGIN_ROOT

PASS=0
FAIL=0
ok() { echo "  PASS  $1"; PASS=$((PASS + 1)); }
bad() { echo "  FAIL  $1"; FAIL=$((FAIL + 1)); }
note() { echo "        $1"; }
has() { case "$1" in *"$2"*) return 0 ;; *) return 1 ;; esac; }

WB="$W/bin"
mkdir -p "$WB"
cat >"$WB/logos" <<EOF
#!/bin/sh
[ "\${1:-}" = --version ] && { echo "logos 1.20.0"; exit 0; }
exec "$LOGOS_BIN" "\$@"
EOF
chmod +x "$WB/logos"
export PATH="$WB:$PATH"
GIT=(git -c user.name=uat -c user.email=uat@uat -c maintenance.auto=false -c gc.auto=0 -c commit.gpgsign=false)

# --- preconditions -------------------------------------------------------------------

echo "preconditions"
F="$W/fixture"
mkdir -p "$F/src" "$F/.logos" "$F/.claude"
cat >"$F/Cargo.toml" <<'EOF'
[package]
name = "uatfx"
version = "0.1.0"
edition = "2021"

[dependencies]
EOF
# The baseline has a spread of complexity, as any real crate does. Equality lists
# every function above the mean cyclomatic complexity; over a baseline of CC-1
# functions alone, any new function with a branch is a `new` Equality entry, which
# blocks the stop by FR-IN-13 (seen on this UAT's first run; see S-683's notes).
cat >"$F/src/lib.rs" <<'RS'
mod util;

#[cfg(test)]
mod tests;

pub fn shallow(x: i32) -> i32 {
    util::inc(x)
}

pub fn classify(x: i32) -> &'static str {
    match x {
        i32::MIN..=-1 => "negative",
        0 => "zero",
        1..=9 => "small",
        _ => "large",
    }
}
RS
cat >"$F/src/util.rs" <<'RS'
pub fn inc(x: i32) -> i32 {
    x + 1
}

pub fn clamp(x: i32, lo: i32, hi: i32) -> i32 {
    if x < lo {
        lo
    } else if x > hi {
        hi
    } else {
        x
    }
}
RS
cat >"$F/src/tests.rs" <<'RS'
use crate::{classify, shallow, util};

#[test]
fn shallow_adds_one() {
    assert_eq!(shallow(1), 2);
}

#[test]
fn classify_buckets() {
    assert_eq!(classify(-3), "negative");
    assert_eq!(classify(0), "zero");
    assert_eq!(classify(4), "small");
    assert_eq!(classify(40), "large");
}

#[test]
fn clamp_bounds() {
    assert_eq!(util::clamp(-1, 0, 9), 0);
    assert_eq!(util::clamp(12, 0, 9), 9);
    assert_eq!(util::clamp(5, 0, 9), 5);
}
RS
(cd "$F" && cargo generate-lockfile --offline >/dev/null 2>&1) || exit 3
printf '[constraints]\nmax_nesting_depth = 4\n' >"$F/.logos/rules.toml"
printf 'target/\n' >"$F/.gitignore"
# A foreign setting, which the uninstall of step 6 must leave alone.
printf '{\n  "permissions": {\n    "allow": ["Bash(ls:*)"]\n  }\n}\n' >"$F/.claude/settings.json"
(cd "$F" && "${GIT[@]}" init -q -b main . && "${GIT[@]}" add -A && "${GIT[@]}" commit -qm base) || exit 3

if [ "${UAT_CLAUDE:-0}" = 1 ]; then
    command -v claude >/dev/null 2>&1 || { echo "UAT_CLAUDE=1 but claude is not on PATH" >&2; exit 3; }
    export CLAUDE_CONFIG_DIR="$W/ccfg"
    mkdir -p "$CLAUDE_CONFIG_DIR"
    (cd "$F" && claude plugin marketplace add "$HERE" --scope project && claude plugin install logos@logos --scope project) \
        >"$W/plugin-install.log" 2>&1 && ok "the plugin installs from the repository marketplace" ||
        { bad "the plugin did not install (see $W/plugin-install.log)"; }
else
    python3 -I - "$F/.claude/settings.json" "$HERE" <<'PY'
import json, sys
p, repo = sys.argv[1], sys.argv[2]
d = json.load(open(p))
# What installing from the repository marketplace writes (and `init -i` too).
d["extraKnownMarketplaces"] = {"logos": {"source": {"source": "github", "repo": "raffaelecamanzo/logos"}}}
d["enabledPlugins"] = {"logos@logos": True}
json.dump(d, open(p, "w"), indent=2)
PY
    note "plugin enabled by writing its project settings (UAT_CLAUDE=1 installs it with claude)"
fi

(cd "$F" && logos init --hooks >"$W/init.log" 2>&1) && ok "logos init --hooks enrols the fixture" ||
    bad "init failed (see $W/init.log)"
(cd "$F" && logos index >/dev/null 2>&1 && logos scan --json >"$W/scan.json" 2>/dev/null)
(cd "$F" && "${GIT[@]}" add -A && "${GIT[@]}" commit -qm enrol) >/dev/null 2>&1
(cd "$F" && logos scan --json >/dev/null 2>&1)
SNAP="$(cd "$F" && logos evolution --json --limit 5 2>/dev/null | python3 -I -c 'import json,sys; print(len(json.load(sys.stdin).get("snapshots",[])))' 2>/dev/null || echo 0)"
[ "${SNAP:-0}" -gt 0 ] && ok "a snapshot is recorded on main ($SNAP)" || bad "no snapshot recorded on main"
(cd "$F" && "${GIT[@]}" checkout -qb feature) || exit 3

# hook NAME PAYLOAD — run a plugin hook as the host does; OUT holds its stdout.
hook() {
    OUT="$(cd "$F" && printf '%s' "$2" | env -u LOGOS_INVOKER CLAUDE_PROJECT_DIR="$F" CLAUDE_PLUGIN_ROOT="$PLUGIN" \
        "$PLUGIN/hooks/$1" 2>"$W/hook.err")"
}
STOP='{"hook_event_name":"Stop","stop_hook_active":false,"session_id":"uat"}'
STOP_ACTIVE='{"hook_event_name":"Stop","stop_hook_active":true,"session_id":"uat"}'

# --- step 1: the readout and the MCP tool list ---------------------------------------

echo "step 1: start a session; the readout and the tool list"
if [ "${UAT_CLAUDE:-0}" = 1 ]; then
    rm -f "$CLAUDE_CONFIG_DIR/mcp-needs-auth-cache.json"
    (cd "$F" && perl "$HERE/scripts/run-bounded.pl" 120 claude -p ok --output-format stream-json --verbose --include-hook-events \
        >"$W/session.jsonl" 2>"$W/session.err") || true
    python3 -I - "$W/session.jsonl" >"$W/step1.txt" <<'PY'
import json, sys, collections
tools, readout = [], False
for line in open(sys.argv[1]):
    try:
        e = json.loads(line)
    except ValueError:
        continue
    if e.get("type") == "system" and e.get("subtype") == "init":
        tools = e.get("tools", [])
    blob = json.dumps(e)
    # The readout itself, never the version handshake (which also says "quality").
    if e.get("hook_event") == "SessionStart" and "logos quality report: signal" in e.get("output", ""):
        readout = True
logos = [t for t in tools if "logos" in t]
dup = [t for t, n in collections.Counter(logos).items() if n > 1]
print(f"tools={len(logos)} duplicates={len(dup)} readout={readout}")
PY
    S1="$(cat "$W/step1.txt")"
    note "$S1"
    has "$S1" "readout=True" && ok "the readout appears" || bad "no readout in the session's hook events"
    if has "$S1" "duplicates=0" && ! has "$S1" "tools=0"; then ok "each logos tool is listed once"; else bad "the tool list: $S1"; fi
else
    note "not run: needs a Claude Code host (UAT_CLAUDE=1)"
    hook session-start.sh '{"hook_event_name":"SessionStart","source":"startup"}'
    has "$OUT" "logos quality report: signal" && ok "the session-start hook prints the readout (hook run directly)" ||
        bad "the session-start hook printed no readout: $OUT"
fi

# --- step 2: a 6-deep function blocks the stop once ---------------------------------

echo "step 2: add a function nested 6 deep and end the turn"
cat >>"$F/src/lib.rs" <<'EOF'

pub fn deep(x: i32) -> i32 {
    if x > 0 {
        if x > 1 {
            if x > 2 {
                if x > 3 {
                    if x > 4 {
                        if x > 5 {
                            return 1;
                        }
                    }
                }
            }
        }
    }
    0
}
EOF
# Imported, then called by name: the graph binds that call, while it leaves a
# `crate::deep(6)` path call unbound and the test would not reach the symbol.
sed -i.bak 's/^use crate::{classify, shallow, util};/use crate::{classify, deep, shallow, util};/' "$F/src/tests.rs" && rm -f "$F/src/tests.rs.bak"
printf '\n#[test]\nfn deep_is_one_above_five() {\n    assert_eq!(deep(6), 1);\n    assert_eq!(deep(5), 0);\n}\n' >>"$F/src/tests.rs"
hook stop.sh "$STOP"
FIRST="$OUT"
DEC="$(printf '%s' "$FIRST" | python3 -I -c 'import json,sys; d=json.loads(sys.stdin.read() or "{}"); print(d.get("decision","none"))' 2>/dev/null)"
note "first stop: decision=$DEC"
if [ "$DEC" != block ]; then
    note "the first stop did not block: $(printf '%s' "$FIRST" | head -c 300)"
    # The before-state is built in the background on a cold cache (S-682); the
    # first stop then reads "not measured". Wait for it and stop again.
    MB="$(git -C "$F" merge-base HEAD main)"
    for _ in $(seq 1 120); do [ -f "$F/.logos/stop-hook/ready-$MB" ] && break; sleep 1; done
    printf '\n' >>"$F/src/lib.rs"
    hook stop.sh "$STOP"
    DEC="$(printf '%s' "$OUT" | python3 -I -c 'import json,sys; d=json.loads(sys.stdin.read() or "{}"); print(d.get("decision","none"))' 2>/dev/null)"
    note "second stop: decision=$DEC"
fi
REASON="$(printf '%s' "$OUT" | python3 -I -c 'import json,sys; print(json.loads(sys.stdin.read() or "{}").get("reason",""))' 2>/dev/null)"
printf '%s\n' "$REASON" >"$W/step2-reason.txt"
# The check leg words it `nests 6 levels deep > max_nesting_depth = 4`, the review
# leg `nesting_depth 6, threshold 4`: either names the value and the threshold.
if [ "$DEC" = block ] && has "$REASON" "deep" && { has "$REASON" "6 levels deep > max_nesting_depth = 4" || has "$REASON" "6, threshold 4"; }; then
    ok "the stop is blocked, naming deep, value 6 and threshold 4"
else
    bad "the block: decision=$DEC reason=$(head -c 400 "$W/step2-reason.txt")"
fi
hook stop.sh "$STOP_ACTIVE"
[ -z "$OUT" ] && ok "blocked once: the continued stop is silent" || bad "the continued stop said: $OUT"

# --- step 3: /logos:review, the fix, the re-measurement ------------------------------

echo "step 3: /logos:review and its verdicts"
R="$W/review"
mkdir -p "$R"
skill() { (cd "$F" && env -u LOGOS_INVOKER LOGOS_PROJECT_DIR="$F" bash "$RS/$1" "${@:2}"); }
skill merge-base.sh >"$R/base.txt" 2>"$R/base.err"
skill review-delta.sh --out "$R/delta.json" >"$R/delta.txt" 2>"$R/delta.err" || note "review-delta exit $?: $(tail -2 "$R/delta.err")"
skill coverage-lookup.sh --delta "$R/delta.json" --out "$R/cov.json" >/dev/null 2>"$R/cov.err"
skill trust-gate.sh --delta "$R/delta.json" --coverage "$R/cov.json" --out "$R/triage.json" >"$R/triage.txt" 2>"$R/triage.err"
note "$(grep -i deep "$R/triage.txt" | head -3)"
ENTRY="$(python3 -I -c '
import json,sys
d = json.load(open(sys.argv[1]))
for e in d.get("entries", d if isinstance(d, list) else []):
    if e.get("name") == "deep":
        print(e["id"], e.get("proposed"), e.get("status"), e.get("after"), e.get("threshold")); break' "$R/triage.json" 2>/dev/null)"
note "triage: $ENTRY"
set -- $ENTRY
EID="${1:-}" PROPOSED="${2:-}"
[ "$PROPOSED" = "fix" ] && ok "the verdict proposed for deep is fix" || bad "the verdict proposed for deep: '${PROPOSED}'"

skill edit-scope.sh begin --state "$R/scope.state" >/dev/null 2>&1
python3 -I - "$F/src/lib.rs" <<'PY'
import re, sys
p = sys.argv[1]
s = open(p).read()
start = s.index("pub fn deep(")
s = s[:start] + "pub fn deep(x: i32) -> i32 {\n    i32::from(x > 5)\n}\n"
open(p, "w").write(s)
PY
TESTS="$(cd "$F" && cargo test --quiet 2>&1)"
P="$(printf '%s\n' "$TESTS" | sed -n 's/.*test result: ok\. \([0-9]*\) passed; \([0-9]*\) failed.*/\1 \2/p' | awk '{p+=$1; f+=$2} END {print p+0, f+0}')"
set -- $P
TP="${1:-0}" TF="${2:-0}"
note "cargo test: $TP passed, $TF failed (of $((TP + TF)))"
[ "$TF" = 0 ] && [ "$TP" -gt 0 ] && ok "the refactor keeps the tests passing" || bad "tests after the refactor: $TP passed, $TF failed"
skill edit-scope.sh check --state "$R/scope.state" --allow src/lib.rs >"$R/scope.txt" 2>&1 &&
    ok "only the fix symbol's file was edited" || bad "edit scope: $(cat "$R/scope.txt")"
skill remeasure.sh --triage "$R/triage.json" --out "$R/after.json" --entry "$EID" >"$R/remeasure.txt" 2>"$R/remeasure.err"
REM="$(cat "$R/remeasure.txt")"
note "remeasure: $REM"
if has "$REM" "proven" || has "$REM" "cleared"; then
    ok "the re-measurement proves the fix"
else
    bad "the re-measurement: $REM $(tail -1 "$R/remeasure.err")"
fi
has "$REM" "cleared" && note "the entry reads cleared, as UAT-IN-06 step 3 states" ||
    note "the entry reads $(printf '%s' "$REM" | grep -o 'cleared\|improved\|resolved' | head -1): UAT-IN-06 step 3 states cleared"
OUTCOME="$(printf '%s' "$REM" | grep -o 'resolved\|improved\|cleared' | head -1)"
python3 -I - "$R/triage.json" "$EID" "${OUTCOME:-cleared}" "$TP" "$TF" >"$R/entries.txt" <<'PY'
import json, sys
triage, fix_id, outcome, tp, tf = sys.argv[1:6]
d = json.load(open(triage))
rows = d.get("entries", d if isinstance(d, list) else [])
def where(e):
    return f"{e['file']}:{e['line']}" if e["file"] and e["line"] is not None else (e["file"] or "-")
# A report names an entry by its subject (file:line name), and rows that share one
# — a symbol's nesting and Equality entries — take one verdict together.
groups = {}
for e in rows:
    groups.setdefault(f"{where(e)} {e['name']}", []).append(e)
for subject, es in groups.items():
    fixed = next((e for e in es if e["id"] == fix_id), None)
    if fixed is not None:
        print(f"ENTRY | fix | {subject} | over its threshold, trusted, reached by src/tests.rs")
        print(f"FIXED | {subject} | remeasured: {outcome} {fixed.get('after')} -> {'unlisted' if outcome == 'cleared' else 'see after.json'} | tests: cargo test | {tp} passed, {tf} failed (of {int(tp) + int(tf)})")
        continue
    final = next((e for e in es if e.get("final")), None)
    if final is not None:
        print(f"ENTRY | {final['proposed']} | {subject} | {final.get('reason') or 'set by the gate'}")
    else:
        # The agent's call on a proposed fix it did not take: the one edit already
        # made rewrote this symbol, so no second edit is warranted.
        print(f"ENTRY | no-action-warranted | {subject} | cost exceeds benefit: the fix of {fix_id} already rewrote this symbol")
PY
{
    cat "$R/base.txt"
    echo
    echo '```'
    cat "$R/entries.txt"
    echo '```'
    echo
    tail -1 "$R/delta.err"
    tail -1 "$R/triage.err"
} >"$R/report.md"
skill check-report.sh --report "$R/report.md" --triage "$R/triage.json" >"$R/check-report.txt" 2>&1 &&
    ok "the report passes the evidence check (no aggregate claim)" || bad "the report check: $(cat "$R/check-report.txt")"
hook stop.sh "$STOP"
# The security leg (S-688, which UAT-IN-06 predates) adds a "not measured" notice
# when no Gitleaks is installed or no scan consent is recorded; it never blocks. The
# quality legs say nothing.
NEXT="$(printf '%s' "$OUT" | python3 -I -c '
import json, sys
raw = sys.stdin.read()
if not raw.strip():
    print("silent"); sys.exit()
d = json.loads(raw)
msg = d.get("systemMessage", "")
if d.get("decision") == "block":
    print("blocked")
elif msg.startswith("logos: this turn is not measured: "):
    # Each unmeasured leg is its own clause, and every one must be the security leg.
    rest = msg[len("logos: this turn is not measured: "):].removesuffix(" The stop proceeds.")
    clauses = [c.strip() for c in rest.split(";") if c.strip()]
    print("security-notice-only" if clauses and all(c.startswith("security") for c in clauses) else "other")
else:
    print("other")')"
case "$NEXT" in
    silent) ok "the next stop is silent" ;;
    security-notice-only) ok "the next stop is silent on quality (the security leg notes it could not scan)"; note "$OUT" ;;
    *) bad "the next stop said: $OUT" ;;
esac

# --- step 4: raising the threshold is held at an ask ---------------------------------

echo "step 4: ask the agent to raise max_nesting_depth"
GP="$(python3 -I -c 'import json,sys; print(json.dumps({"hook_event_name":"PreToolUse","cwd":sys.argv[1],"tool_name":"Edit","tool_input":{"file_path":sys.argv[1]+"/.logos/rules.toml","old_string":"max_nesting_depth = 4","new_string":"max_nesting_depth = 6"}}))' "$F")"
hook guard.sh "$GP"
D="$(printf '%s' "$OUT" | python3 -I -c 'import json,sys; d=json.loads(sys.stdin.read() or "{}").get("hookSpecificOutput",{}); print(d.get("permissionDecision","none"), "|", d.get("permissionDecisionReason",""))' 2>/dev/null)"
note "$D"
if has "$D" "ask |" && has "$D" "rules.toml"; then ok "the edit is held at an ask that names rules.toml"; else bad "the guard said: $D"; fi

# --- step 5: stats --json shows the hook and skill counts ----------------------------

echo "step 5: logos stats --json"
(cd "$F" && env -u LOGOS_INVOKER logos stats --json >"$W/stats.json" 2>/dev/null)
python3 -I - "$W/stats.json" >"$W/step5.txt" <<'PY'
import json, sys, collections
d = json.load(open(sys.argv[1]))
by = collections.Counter()
for c in d["calls_by_tool_invoker"]:
    by[c["invoker"] or "null"] += c["calls"]
for k in sorted(by):
    print(f"{k}={by[k]}")
print("cells:", "; ".join(f'{c["tool"]}/{c["invoker"] or "null"}={c["calls"]}' for c in d["calls_by_tool_invoker"]))
print("null_invoker_includes_pre_stamp_rows=" + str(d["attribution_coverage"]["null_invoker_includes_pre_stamp_rows"]))
PY
while IFS= read -r l; do note "$l"; done <"$W/step5.txt"
grep -q '^hook:stop=[1-9]' "$W/step5.txt" && ok "stats shows hook:stop counts" || bad "no hook:stop count"
grep -q '^skill:logos-review=[1-9]' "$W/step5.txt" && ok "stats shows skill:logos-review counts" || bad "no skill:logos-review count"
grep -q 'null_invoker_includes_pre_stamp_rows=True' "$W/step5.txt" && ok "stats states the cross-tab's limits" || bad "no coverage statement"

# --- step 6: uninstall ---------------------------------------------------------------

echo "step 6: logos init --uninstall"
(cd "$F" && logos init --uninstall >"$W/uninstall.log" 2>&1) || bad "uninstall failed (see $W/uninstall.log)"
while IFS= read -r l; do note "$l"; done < <(grep -E 'Removed|Unchanged|Kept' "$W/uninstall.log" | head -20)
python3 -I - "$F" >"$W/step6.txt" <<'PY'
import json, os, sys
f = sys.argv[1]
p = os.path.join(f, ".claude", "settings.json")
d = json.load(open(p)) if os.path.exists(p) else {}
print("foreign=" + str(d.get("permissions", {}).get("allow") == ["Bash(ls:*)"]))
print("plugin=" + str("logos@logos" in d.get("enabledPlugins", {})))
# The marketplace entry goes only when it is exactly what init writes; a local
# `directory` source (UAT_CLAUDE=1 adds this checkout) is the user's and stays.
m = d.get("extraKnownMarketplaces", {}).get("logos")
print("marketplace=" + str(m is not None and m.get("source", {}).get("source") == "github"))
claude_md = os.path.join(f, "CLAUDE.md")
print("claude_md_block=" + str(os.path.exists(claude_md) and "logos" in open(claude_md).read().lower()))
print("mcp_json=" + str(os.path.exists(os.path.join(f, ".mcp.json"))))
PY
HOOKS="$(git -C "$F" config core.hooksPath || true)"
note "$(tr '\n' ' ' <"$W/step6.txt") core.hooksPath=${HOOKS:-unset}"
grep -q 'foreign=True' "$W/step6.txt" && ok "foreign settings are intact" || bad "the foreign setting was changed"
if grep -q 'plugin=False' "$W/step6.txt" && grep -q 'marketplace=False' "$W/step6.txt" &&
    grep -q 'claude_md_block=False' "$W/step6.txt" &&
    grep -q 'mcp_json=False' "$W/step6.txt" && [ -z "$HOOKS" ]; then
    ok "every Logos-written agent-host artefact is removed"
else
    bad "artefacts left: $(tr '\n' ' ' <"$W/step6.txt") core.hooksPath=${HOOKS:-unset}"
fi

echo "$PASS passed, $FAIL failed (of $((PASS + FAIL))) — work directory $W"
[ "$FAIL" -eq 0 ]
