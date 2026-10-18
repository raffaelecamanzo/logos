#!/usr/bin/env bash
#
# test-plugin-guard.sh — prove the logos plugin's PreToolUse guardrail
# (plugins/logos/hooks/guard.sh, S-680, FR-IN-14, BR-69) asks the human before an
# agent relaxes the quality contract, and lets everything else through.
#
# The hook runs as the host runs it: executed directly, under `env -i`, with a
# PATH holding only `bash`, so it is proved to need nothing but bash builtins.
# Every case runs under each distinct bash on this machine (/bin/bash is 3.2 on
# macOS).
#
# Layers:
#   1. the six guarded actions, one fixture each, the reason naming its target;
#   2. other spellings of the same actions: the MCP twin under both tool
#      prefixes, other file tools, path spellings, shell writes and invocations;
#   3. pass-through: unguarded .logos/ edits, reads of guarded files, unrelated
#      commands, and the near miss of each matcher;
#   4. a malformed payload asks; an unenrolled project is silent;
#   5. hooks.json registers the guard for every tool it guards, and its command
#      runs from a plugin root with a space in it.
#
# Usage: bash scripts/tests/test-plugin-guard.sh
# Exit:  0 all cases behaved, 1 a case did not

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PLUGIN="$HERE/plugins/logos"
HOOK="$PLUGIN/hooks/guard.sh"

WORK="${TMPDIR:-/tmp}/logos-plugin-guard-test.$$"
trap 'rm -rf "$WORK"' EXIT INT TERM
mkdir -p "$WORK" || exit 1
WORK="$(cd "$WORK" && pwd -P)"
export GIT_CEILING_DIRECTORIES="$WORK"

PASS=0
FAIL=0
ok() { echo "  ok    $1"; PASS=$((PASS + 1)); }
bad() { echo "  FAIL  $1"; FAIL=$((FAIL + 1)); }
expect() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1: got '$2', wanted '$3'"; fi; }

for f in "$HOOK" "$PLUGIN/hooks/lib/common.sh" "$PLUGIN/hooks/hooks.json"; do
    [ -f "$f" ] || { echo "FAIL  missing $f"; echo "0 passed, 1 failed (of 1)"; exit 1; }
done

# Each distinct bash on this machine.
BASHES=()
for b in /bin/bash "$(command -v bash)"; do
    [ -x "$b" ] || continue
    r="$(cd "$(dirname "$b")" && pwd -P)/$(basename "$b")"
    dup=no
    for s in "${BASHES[@]+"${BASHES[@]}"}"; do [ "$s" = "$r" ] && dup=yes; done
    [ "$dup" = yes ] || BASHES+=("$r")
done

ENROLLED="$WORK/enrolled"
PLAIN="$WORK/plain"
mkdir -p "$ENROLLED/.logos" "$PLAIN"

# json STRING — STRING as a JSON string literal, for building payloads.
json() { python3 -I -c 'import json,sys; print(json.dumps(sys.argv[1]))' "$1"; }
file_payload() { # tool key path
    printf '{"session_id":"t","hook_event_name":"PreToolUse","tool_name":"%s","tool_input":{"%s":%s,"content":"x"}}' \
        "$1" "$2" "$(json "$3")"
}
bash_payload() { # command
    printf '{"session_id":"t","hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":%s,"description":"d"}}' \
        "$(json "$1")"
}
mcp_payload() { # tool
    printf '{"session_id":"t","hook_event_name":"PreToolUse","tool_name":"%s","tool_input":{}}' "$1"
}

# run <bash> <project> — the payload on stdin; sets OUT, ERR, RC.
run() {
    local sys="$WORK/sys-${1//\//_}"
    mkdir -p "$sys" && ln -sf "$1" "$sys/bash"
    RC=0
    (cd "$2" && env -i HOME="$WORK" PATH="$sys" CLAUDE_PLUGIN_ROOT="$PLUGIN" CLAUDE_PROJECT_DIR="$2" \
        ${RUN_LANG:+LANG="$RUN_LANG"} "$HOOK") >"$WORK/out" 2>"$WORK/err" || RC=$?
    OUT="$(cat "$WORK/out")" ERR="$(cat "$WORK/err")"
}

# decision — "ask|<reason>" when OUT is one valid PreToolUse ask object, "pass"
# when OUT is empty, INVALID otherwise.
decision() {
    printf '%s' "$OUT" | python3 -I -c '
import json, sys
s = sys.stdin.read()
if s == "":
    print("pass"); sys.exit()
try:
    d = json.loads(s)
    h = d["hookSpecificOutput"]
    assert set(d) == {"hookSpecificOutput"} and h["hookEventName"] == "PreToolUse"
    assert h["permissionDecision"] == "ask" and h["permissionDecisionReason"]
except Exception:
    print("INVALID"); sys.exit()
print("ask|" + h["permissionDecisionReason"])'
}

# asks <label> <payload> <target> — the guard asks, exit 0, stderr empty, and
# the reason names <target>. The malformed-payload reason lists every guarded
# file, so it would name any target: it counts only where it is the one wanted.
# passes <label> <payload> [project] — exit 0, no output.
B=""
T=""
asks() {
    local d
    printf '%s' "$2" >"$WORK/in"
    run "$B" "$ENROLLED" <"$WORK/in"
    d="$(decision)"
    case "$3:$d" in
        "could not be read:"*) ;;
        *"could not be read"*) d="malformed instead: $d" ;;
    esac
    case "$d" in
        "ask|"*"$3"*)
            if [ "$RC|$ERR" = "0|" ]; then ok "$T $1"; else bad "$T $1: exit $RC, stderr '$ERR'"; fi
            ;;
        *) bad "$T $1: wanted an ask naming '$3', got '$d' (exit $RC, stderr '$ERR')" ;;
    esac
}
passes() {
    printf '%s' "$2" >"$WORK/in"
    run "$B" "${3:-$ENROLLED}" <"$WORK/in"
    expect "$T $1" "$RC|$OUT|$ERR" "0||"
}

# bounded asks|passes <label> <payload-file> [target] — the case, decided in at
# most BOUND seconds (well inside the 10 s hooks.json timeout).
BOUND=5
bounded() {
    local kind="$1" label="$2" file="$3" start=$SECONDS
    "$kind" "$label" "$(cat "$file")" ${4+"$4"}
    expect "$T ... decided within ${BOUND}s" "$([ $((SECONDS - start)) -le "$BOUND" ] && echo yes || echo "no ($((SECONDS - start))s)")" yes
}
python3 -I - "$WORK" <<'PY'
import json, sys, os
w = sys.argv[1]
body = "\n".join(" ".join("word%d" % (i * 8 + j) for j in range(8)) for i in range(375))
def save(name, tool, tool_input):
    # Raw UTF-8, as the host sends it: the guard does not decode \u escapes.
    with open(os.path.join(w, name), "w", encoding="utf-8") as f:
        json.dump({"session_id": "t", "hook_event_name": "PreToolUse", "tool_name": tool, "tool_input": tool_input},
                  f, ensure_ascii=False)
save("long-rm.json", "Bash", {"command": "cat > notes.txt <<EOF\n" + body + "\nEOF\nrm .logos/rules.toml"})
save("long-benign.json", "Bash", {"command": "git commit -F - <<EOF\n" + body + "\nEOF"})
save("big-write.json", "Write", {"content": "x = \"y\"\n" * 120000, "file_path": ".logos/rules.toml"})
save("big-edit.json", "Edit", {"old_string": "a \"quoted\" line\n" * 15000, "new_string": "b", "file_path": ".logos/rules-baseline.json"})
save("quotes.json", "Bash", {"command": "printf %s " + "x\"y" * 8000})
save("multibyte-rm.json", "Bash", {"command": "echo " + "\u00e0b " * 6000 + "&& rm .logos/rules.toml"})
save("utf8-path.json", "Write", {"file_path": "/tmp/citt\u00e0/.logos/rules.toml"})
save("long-path.json", "Write", {"file_path": "/tmp/" + "\u00e0" * 1500 + "/.logos/rules.toml"})
PY

for B in "${BASHES[@]}"; do
    # shellcheck disable=SC2016 # expanded by the bash under test
    T="[bash $("$B" -c 'echo "${BASH_VERSINFO[0]}.${BASH_VERSINFO[1]}"')]"

    # --- 1. the six guarded actions ---------------------------------------------
    asks "Write .logos/rules.toml asks, naming the file" \
        "$(file_payload Write file_path "$ENROLLED/.logos/rules.toml")" ".logos/rules.toml"
    asks "Edit .logos/rules-baseline.json asks, naming the file" \
        "$(file_payload Edit file_path "$ENROLLED/.logos/rules-baseline.json")" ".logos/rules-baseline.json"
    asks "Write .logos/security-ignore.toml asks, naming the file" \
        "$(file_payload Write file_path "$ENROLLED/.logos/security-ignore.toml")" ".logos/security-ignore.toml"
    asks "Bash 'logos gate --save' asks, naming the command" \
        "$(bash_payload 'logos gate --save --label release')" "logos gate --save"
    asks "Bash 'logos check --bless' asks, naming the command" \
        "$(bash_payload 'logos check --bless')" "logos check --bless"
    asks "Bash 'logos security ingest' asks, naming the command" \
        "$(bash_payload 'logos security ingest empty.sarif')" "logos security ingest"

    # --- 2. other spellings of the same actions ---------------------------------
    asks "the MCP twin under the plugin prefix asks, naming the command" \
        "$(mcp_payload mcp__plugin_logos_logos__bless_rules)" "logos check --bless"
    asks "the MCP twin under the project .mcp.json prefix asks" \
        "$(mcp_payload mcp__logos__bless_rules)" "logos check --bless"
    asks "the security-ingest MCP tool under the plugin prefix asks, naming the command" \
        "$(mcp_payload mcp__plugin_logos_logos__security_ingest)" "logos security ingest"
    asks "the security-ingest MCP tool under the project .mcp.json prefix asks" \
        "$(mcp_payload mcp__logos__security_ingest)" "logos security ingest"
    # shellcheck disable=SC2088 # a literal tilde, as an agent writes it
    asks "logos security ingest by path, with a global flag and --json" \
        "$(bash_payload '~/.logos-bin/1.20.0/logos --json security ingest --json empty.sarif')" "logos security ingest"
    asks "logos security ingest behind env and an assignment" \
        "$(bash_payload 'FOO=1 env logos security ingest --project . r.sarif')" "logos security ingest"
    asks "logos security ingest after another command in a list" \
        "$(bash_payload 'cargo build && logos security ingest r.sarif')" "logos security ingest"
    # shellcheck disable=SC2016 # a literal $L, for the guard to read
    asks "logos security ingest held in a variable, with a global option whose value is logos" \
        "$(bash_payload 'L=logos; $L security --project logos ingest r.sarif')" "logos security ingest"
    asks "logos security ingest with a global option whose value is logos" \
        "$(bash_payload 'logos security --project logos ingest r.sarif')" "logos security ingest"
    asks "logos security ingest inside bash -c" \
        "$(bash_payload 'bash -c "logos security ingest r.sarif"')" "logos security ingest"
    asks "a line continuation inside logos security ingest asks" "$(bash_payload 'logos security \
    ingest r.sarif')" "logos security ingest"
    asks "a multi-line command ending in logos security ingest" "$(bash_payload 'git status
logos security ingest r.sarif')" "logos security ingest"
    # shellcheck disable=SC2016 # literal substitutions, for the guard to read
    asks "logos security ingest found by \$(which logos)" \
        "$(bash_payload '$(which logos) security ingest r.sarif')" "logos security ingest"
    # shellcheck disable=SC2016
    asks "logos security ingest found by \`command -v logos\`" \
        "$(bash_payload '`command -v logos` security ingest r.sarif')" "logos security ingest"
    # shellcheck disable=SC2016
    asks "logos security ingest held in a variable" \
        "$(bash_payload 'L=logos; $L security ingest r.sarif')" "logos security ingest"
    asks "MultiEdit of a guarded file asks" \
        "$(file_payload MultiEdit file_path "$ENROLLED/.logos/rules.toml")" ".logos/rules.toml"
    asks "NotebookEdit's notebook_path is read" \
        "$(file_payload NotebookEdit notebook_path ".logos/rules-baseline.json")" ".logos/rules-baseline.json"
    passes "NotebookEdit of an unguarded notebook passes" "$(file_payload NotebookEdit notebook_path "nb/analysis.ipynb")"
    asks "a relative path asks" "$(file_payload Edit file_path ".logos/rules.toml")" ".logos/rules.toml"
    asks "a workspace member's contract asks" \
        "$(file_payload Edit file_path "$ENROLLED/web/.logos/rules.toml")" ".logos/rules.toml"
    asks "'./.logos//rules.toml' is the same file" "$(file_payload Write file_path "./.logos//rules.toml")" ".logos/rules.toml"
    asks "'.logos/x/../rules.toml' is the same file" \
        "$(file_payload Write file_path ".logos/x/../rules.toml")" ".logos/rules.toml"
    asks "a case-folded '.LOGOS/Rules.TOML' is the same file on a case-folding disk" \
        "$(file_payload Write file_path ".LOGOS/Rules.TOML")" ".logos/rules.toml"
    asks "a JSON-escaped '\\/' path is decoded" \
        '{"tool_name":"Write","tool_input":{"file_path":"a\\\\b\/.logos\/rules.toml"}}' ".logos/rules.toml"
    asks "a redirection into a guarded file asks" \
        "$(bash_payload 'echo "[constraints]" > .logos/rules.toml')" ".logos/rules.toml"
    asks "an append redirection asks" "$(bash_payload 'printf x >>.logos/security-ignore.toml')" ".logos/security-ignore.toml"
    asks "an &> redirection asks" "$(bash_payload 'make &>.logos/rules.toml')" ".logos/rules.toml"
    asks "a quoted, concatenated path asks" "$(bash_payload "echo x > '.logos/'rules.toml")" ".logos/rules.toml"
    asks "tee asks" "$(bash_payload 'echo x | tee -a .logos/rules-baseline.json')" ".logos/rules-baseline.json"
    asks "sed -i asks" "$(bash_payload 'sed -i.bak s/a/b/ .logos/rules.toml')" ".logos/rules.toml"
    asks "sed --in-place asks" "$(bash_payload 'sed --in-place s/a/b/ .logos/rules.toml')" ".logos/rules.toml"
    asks "BSD sed -I asks" "$(bash_payload 'sed -I "" s/a/b/ .logos/rules.toml')" ".logos/rules.toml"
    asks "sed's w command asks" "$(bash_payload "sed -n '/max/w .logos/rules.toml' notes.txt")" ".logos/rules.toml"
    asks "sed's s///w flag asks" "$(bash_payload "sed 's/a/b/w .logos/rules-baseline.json' notes.txt")" ".logos/rules-baseline.json"
    asks "xxd -r asks" "$(bash_payload 'xxd -r dump.hex .logos/rules.toml')" ".logos/rules.toml"
    asks "less -o asks" "$(bash_payload 'less -o .logos/rules.toml notes.txt')" ".logos/rules.toml"
    asks "git diff --output asks" "$(bash_payload 'git diff --output=.logos/rules.toml HEAD~1')" ".logos/rules.toml"
    asks "rm asks" "$(bash_payload 'rm -f .logos/rules-baseline.json')" ".logos/rules-baseline.json"
    asks "find -delete asks" "$(bash_payload 'find .logos -name rules.toml -delete')" ".logos"
    asks "cp onto a guarded file asks" "$(bash_payload 'cp /tmp/loose.toml .logos/rules.toml')" ".logos/rules.toml"
    asks "git checkout of a guarded file asks" \
        "$(bash_payload 'git checkout HEAD~3 -- .logos/rules.toml')" ".logos/rules.toml"
    asks "git restore of a guarded file asks" "$(bash_payload 'git restore --source=HEAD~2 .logos/rules.toml')" ".logos/rules.toml"
    asks "git rm of a guarded file asks" "$(bash_payload 'git rm .logos/security-ignore.toml')" ".logos/security-ignore.toml"
    asks "rm -rf of the .logos directory asks" "$(bash_payload 'rm -rf .logos')" ".logos"
    asks "a glob that matches a guarded name asks" "$(bash_payload 'rm .logos/*.json')" ".logos/rules-baseline.json"
    asks "a ? glob that matches a guarded name asks" "$(bash_payload 'rm .logos/rules.tom?')" ".logos/rules.toml"
    asks "a brace list under .logos/ asks" "$(bash_payload 'rm .logos/{rules.toml,x}')" ".logos/"
    asks "a bare guarded name after cd .logos asks" \
        "$(bash_payload 'cd .logos && rm security-ignore.toml')" ".logos/security-ignore.toml"
    asks "a path glued to dd's of= asks" "$(bash_payload 'dd if=/tmp/loose of=.logos/rules.toml')" ".logos/rules.toml"
    asks "a path glued to a short option asks" "$(bash_payload 'sort -o.logos/rules.toml /tmp/loose')" ".logos/rules.toml"
    asks "a path glued to a long option asks" \
        "$(bash_payload 'curl --output=.logos/rules-baseline.json https://example.invalid/b')" ".logos/rules-baseline.json"
    asks "the .logos directory glued to an option asks" "$(bash_payload 'cp --target-directory=.logos /tmp/rules.toml')" ".logos"
    asks "a bare glob after cd .logos asks" "$(bash_payload 'cd .logos && rm -f rules*')" ".logos/rules.toml"
    asks "a bare * after cd .logos asks" "$(bash_payload 'cd .logos; rm *')" ".logos/"
    asks "a bare brace list after cd .logos asks" "$(bash_payload 'cd .logos && rm {rules.toml,x}')" ".logos"
    asks "a bare guarded name after a quoted cd into .logos asks" \
        "$(bash_payload 'cd ".lo"gos && rm rules-baseline.json')" ".logos/rules-baseline.json"
    # shellcheck disable=SC2016 # literal backticks, for the guard to read
    asks "a write inside backticks asks" "$(bash_payload 'echo `rm .logos/rules.toml`')" ".logos/rules.toml"
    # shellcheck disable=SC2016 # a literal $( ), for the guard to read
    asks "a write inside \$( ) asks" "$(bash_payload 'echo $(rm .logos/rules.toml)')" ".logos/rules.toml"
    # shellcheck disable=SC2088 # a literal tilde, as an agent writes it
    asks "logos by path, with a global flag first" \
        "$(bash_payload '~/.logos-bin/1.20.0/logos --json gate --save')" "logos gate --save"
    asks "logos behind env and an assignment" "$(bash_payload 'FOO=1 env logos check --rules r.toml --bless')" \
        "logos check --bless"
    asks "logos after another command in a list" "$(bash_payload 'cargo build && logos check --bless')" \
        "logos check --bless"
    asks "logos inside bash -c" "$(bash_payload 'bash -c "logos gate --save"')" "logos gate --save"
    asks "a line continuation inside logos gate --save asks" "$(bash_payload 'logos gate \
    --save')" "logos gate --save"
    asks "a line continuation after logos asks" "$(bash_payload 'logos \
    check --bless')" "logos check --bless"
    asks "an escaped backslash before a newline is no continuation: the next line is its own command" \
        "$(bash_payload 'echo a\\
rm .logos/rules.toml')" ".logos/rules.toml"
    asks "a multi-line command" "$(bash_payload 'git status
logos gate --save')" "logos gate --save"
    # The binary named in another simple command than the subcommand.
    # shellcheck disable=SC2016 # literal substitutions, for the guard to read
    asks "logos found by \$(which logos)" "$(bash_payload '$(which logos) gate --save')" "logos gate --save"
    # shellcheck disable=SC2016
    asks "logos found by \`command -v logos\`" "$(bash_payload '`command -v logos` check --bless')" "logos check --bless"
    # shellcheck disable=SC2016
    asks "logos held in a variable" "$(bash_payload 'L=logos; $L gate --save')" "logos gate --save"

    # --- 3. pass-through --------------------------------------------------------
    passes "an Edit of an unguarded .logos/ file passes" \
        "$(file_payload Edit file_path "$ENROLLED/.logos/config.toml")"
    passes "a Write of an unguarded .logos/ file passes" "$(file_payload Write file_path ".logos/hooks/x.sh")"
    passes "a Read of a guarded file passes" "$(file_payload Read file_path "$ENROLLED/.logos/rules.toml")"
    passes "a Grep of a guarded file passes" \
        '{"tool_name":"Grep","tool_input":{"pattern":"max_cc","path":".logos/rules.toml"}}'
    passes "cat of a guarded file passes" "$(bash_payload 'cat .logos/rules.toml')"
    passes "grep of a guarded file, redirected elsewhere, passes" \
        "$(bash_payload 'grep -n max .logos/rules.toml > /tmp/out.txt 2>&1')"
    passes "git diff of a guarded file passes" "$(bash_payload 'git -C . diff -- .logos/rules-baseline.json')"
    passes "git add of a guarded file passes (it records, never writes, the file)" "$(bash_payload 'git add .logos/rules.toml')"
    passes "a continued cat of a guarded file passes" "$(bash_payload 'cat \
    .logos/rules.toml')"
    passes "sed without -i passes" "$(bash_payload 'sed -n 1,5p .logos/rules.toml')"
    passes "find without -delete passes" "$(bash_payload 'find .logos -name rules.toml')"
    passes "ls of the .logos directory passes" "$(bash_payload 'ls -la .logos')"
    passes "a guarded file read from with < passes" "$(bash_payload 'python3 x.py < .logos/rules.toml')"
    passes "logos check with --rules naming the contract passes" \
        "$(bash_payload 'logos check --rules .logos/rules.toml')"
    passes "logos gate without --save passes" "$(bash_payload 'logos gate --threshold 10')"
    passes "an unrelated command passes" "$(bash_payload 'cargo test -p logos-core && git status')"
    passes "an unrelated MCP tool passes" "$(mcp_payload mcp__plugin_logos_logos__check_rules)"
    passes "an unknown tool passes" '{"tool_name":"WebFetch","tool_input":{"url":"https://example.invalid"}}'
    # near misses
    passes "near miss: .logos/rules.toml.bak passes" "$(file_payload Write file_path ".logos/rules.toml.bak")"
    passes "near miss: x.logos/rules.toml passes" "$(file_payload Write file_path "x.logos/rules.toml")"
    passes "near miss: a root rules.toml passes" "$(file_payload Write file_path "$ENROLLED/rules.toml")"
    passes "near miss: logos/rules.toml (no dot) passes" "$(file_payload Write file_path "logos/rules.toml")"
    passes "near miss: a glued path read by a reader passes" "$(bash_payload 'grep --file=.logos/rules.toml x.txt')"
    passes "near miss: a glued path to an unguarded name passes" "$(bash_payload 'make OUT=.logos/rules.toml.bak')"
    passes "near miss: a glob that cannot match a guarded name passes" "$(bash_payload 'rm -f .logos/logos.db*')"
    passes "near miss: a bare glob after cd .logos that matches no guarded name passes" \
        "$(bash_payload 'cd .logos && rm -f *.db-wal')"
    passes "near miss: a bare * with no .logos in the command passes" "$(bash_payload 'cd build && rm -f *')"
    passes "near miss: a bare guarded name with no .logos in the command passes" "$(bash_payload 'rm rules.toml')"
    passes "near miss: 'gate --save' without logos passes" "$(bash_payload 'echo gate --save')"
    passes "near miss: '--save' before gate passes" "$(bash_payload 'logos --save gate')"
    passes "near miss: '--bless' without check passes" "$(bash_payload 'logos scan --bless')"
    passes "near miss: '--save' in another command of the list passes" "$(bash_payload 'logos gate; npm --save i x')"
    passes "near miss: 'logos security findings' passes" "$(bash_payload 'logos security findings --json')"
    # These name a token the early-exit filter lets through, so they reach the word walk.
    passes "near miss: 'security findings --save' reaches the walk and passes" "$(bash_payload 'logos security findings --save')"
    passes "near miss: 'security status --bless' reaches the walk and passes" "$(bash_payload 'logos security status --bless')"
    passes "near miss: 'security scan --save' reaches the walk and passes" "$(bash_payload 'logos security scan --path src --save')"
    passes "near miss: 'logos security status' passes" "$(bash_payload 'logos security status')"
    passes "near miss: 'logos security scan' passes" "$(bash_payload 'logos security scan --path src')"
    passes "near miss: 'ingest' before security passes" "$(bash_payload 'logos ingest security')"
    passes "near miss: 'security ingest' without logos passes" "$(bash_payload 'echo security ingest r.sarif')"
    passes "near miss: 'ingest' in another command of the list passes" "$(bash_payload 'logos security status; npm ingest x')"
    passes "near miss: security_ingest under a foreign server passes" "$(mcp_payload mcp__other__security_ingest)"
    passes "near miss: the other security MCP tools pass" "$(mcp_payload mcp__plugin_logos_logos__security_findings)"
    passes "near miss: bless_rules under a foreign server passes" "$(mcp_payload mcp__other__bless_rules)"
    passes "near miss: a guarded path inside another file's content passes" \
        '{"tool_name":"Write","tool_input":{"file_path":"notes.md","content":"see \"file_path\":\".logos/rules.toml\""}}'

    # --- 4. malformed payloads, and an unenrolled project -----------------------
    for m in "empty|" "not JSON|garbage" "a JSON array|[]" "no tool_name|{\"tool_input\":{}}" \
        "Write with no file_path|{\"tool_name\":\"Write\",\"tool_input\":{\"content\":\"x\"}}" \
        "Bash with no command|{\"tool_name\":\"Bash\",\"tool_input\":{}}" \
        "a truncated payload|{\"tool_name\":\"Edit\",\"tool_input\":{\"file_path\":\".logos/con"; do
        asks "malformed payload (${m%%|*}) asks" "${m#*|}" "could not be read"
    done
    passes "an unenrolled project is silent, even for a guarded write" \
        "$(file_payload Write file_path "$PLAIN/.logos/rules.toml")" "$PLAIN"
    passes "an unenrolled project is silent, even for a malformed payload" "garbage" "$PLAIN"

    # --- 6. bounded time -------------------------------------------------------
    # The host cancels a hook at its timeout and lets the call through, so a slow
    # guard is an open guard. Each case must decide well inside hooks.json's 10 s.
    asks "every file_path in the payload is read, not only the first" \
        '{"tool_name":"Write","tool_input":{"file_path":"notes.md","file_path":".logos/rules.toml"}}' ".logos/rules.toml"
    bounded asks "a 3000-word heredoc, then a guarded rm: asks" "$WORK/long-rm.json" ".logos/rules.toml"
    bounded passes "a 3000-word heredoc that names nothing guarded: passes" "$WORK/long-benign.json"
    bounded asks "a 1 MB Write whose content precedes file_path: asks" "$WORK/big-write.json" ".logos/rules.toml"
    bounded asks "a 240 KB Edit, escaped quotes, old_string before file_path: asks" "$WORK/big-edit.json" \
        ".logos/rules-baseline.json"
    # bash 3.2's ${var//pat/rep} is far worse than linear once matches are many,
    # and slower again on multibyte text under a UTF-8 locale.
    bounded passes "a command of 8000 quotes that names nothing guarded: passes" "$WORK/quotes.json"
    RUN_LANG=en_US.UTF-8 bounded asks "6000 multibyte words, then a guarded rm, under a UTF-8 locale: asks" \
        "$WORK/multibyte-rm.json" ".logos/rules.toml"
    RUN_LANG=en_US.UTF-8 asks "a UTF-8 path is quoted in a valid JSON reason" "$(cat "$WORK/utf8-path.json")" \
        "citt$(printf '\303\240')/.logos/rules.toml"
    asks "an over-long path still gives one valid JSON object, its reason cut" "$(cat "$WORK/long-path.json")" "would write"
done

# --- 5. hooks.json ----------------------------------------------------------------

B="${BASHES[0]}"
REG="$(python3 -I - "$PLUGIN/hooks/hooks.json" <<'PY'
import json, re, sys
groups = json.load(open(sys.argv[1]))["hooks"].get("PreToolUse", [])
hits = [(g.get("matcher", ""), h) for g in groups for h in g["hooks"] if h["command"].endswith('/hooks/guard.sh"')]
if len(hits) != 1:
    print("registered %d times" % len(hits)); sys.exit()
m, h = hits[0]
want = ["Write", "Edit", "MultiEdit", "NotebookEdit", "Bash",
        "mcp__logos__bless_rules", "mcp__plugin_logos_logos__bless_rules",
        "mcp__logos__security_ingest", "mcp__plugin_logos_logos__security_ingest"]
miss = [t for t in want if not re.fullmatch(m, t)]
extra = [t for t in ["Read", "Grep", "Glob", "mcp__logos__check_rules", "mcp__other__bless_rules",
    "mcp__logos__security_findings", "mcp__other__security_ingest"] if re.fullmatch(m, t)]
print("missing %s" % miss if miss else "matches %s" % extra if extra else
      "ok %s %s" % (h["type"], h["command"]))
PY
)"
expect "[manifest] hooks.json registers guard.sh once, for every guarded tool and no reader" \
    "${REG%% *}" "ok"
CMD="${REG#ok command }"

SPACED="$WORK/plugin root"
mkdir -p "$SPACED" && cp -R "$PLUGIN/." "$SPACED/"
SYS="$WORK/sys-cmd" && mkdir -p "$SYS" && ln -sf "$B" "$SYS/bash"
RC=0
OUT="$(cd "$ENROLLED" && bash_payload 'logos check --bless' | env -i HOME="$WORK" PATH="$SYS" \
    CLAUDE_PLUGIN_ROOT="$SPACED" CLAUDE_PROJECT_DIR="$ENROLLED" /bin/sh -c "$CMD" 2>"$WORK/err")" || RC=$?
expect "[manifest] the hooks.json command runs from a plugin root with a space: it asks" \
    "$RC|$(decision | cut -c1-4)" "0|ask|"
expect "[manifest] ... with nothing on stderr" "$(cat "$WORK/err")" ""

echo
echo "$PASS passed, $FAIL failed (of $((PASS + FAIL)))"
[ "$FAIL" -eq 0 ] || exit 1
exit 0
